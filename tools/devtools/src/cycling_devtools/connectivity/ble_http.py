#!/usr/bin/python3
"""Send HTTP requests to a paired C606 over its encrypted Bluetooth link, as the phone does.

Pair once with `cycling pair`, which saves the key this tool reads. A paired
device advertises its sync service whenever it is on and not recording, with
its pairing id in the scan response's service data; the tool picks the device
whose id matches (or takes --address). It opens an LE L2CAP channel on PSM
0x0081, runs the protocol v2 handshake (vana_link.py) and sends each request
as sealed frames split into small SDUs, then reads the sealed response frames
up to the empty one that ends it. Every request goes over the same channel, one
at a time. Afterwards it drops the link, as a phone that leaves does, which
ends the device's Bluetooth session (--keep keeps it).

A request is METHOD PATH, optionally followed by a body: inline text, or
@FILE for a file's bytes.

    cycling ble-http GET /api/status GET /api/activities GET /api/workouts
    cycling ble-http POST /api/wifi/scan GET /api/wifi PUT /api/wifi '{"ssid":"x","password":"y"}'
    cycling ble-http --wrong-key GET /api/status   # the device must close the channel
"""

import argparse
import ctypes
import json
import os
import socket
import struct
import sys
import time
from pathlib import Path

from cycling_devtools.connectivity.vana_link import (
    NONCE_BYTES,
    TAG_BYTES,
    AuthenticationError,
    Link,
    Pairing,
    frame,
    load_pairing,
    take_frame,
)

SYNC_SERVICE = '56414e41-0001-4c69-6e6b-c606c606c606'
PSM = 0x0081
METHODS = ('GET', 'HEAD', 'PUT', 'POST', 'DELETE', 'OPTIONS')
# Ciphertext bytes per phone frame, tag included.
PHONE_FRAME = 1024
# Seconds to wait for a device whose service data carries our id before taking one
# that advertises the sync service with no service data.
UUID_FALLBACK_S = 4

AF_BLUETOOTH = 31
BTPROTO_L2CAP = 0
SOL_BLUETOOTH = 274
BT_RCVMTU = 13
BDADDR_LE_PUBLIC = 1
BDADDR_LE_RANDOM = 2


class SockaddrL2(ctypes.Structure):
    _fields_ = [
        ('family', ctypes.c_ushort),
        ('psm', ctypes.c_ushort),
        ('bdaddr', ctypes.c_ubyte * 6),
        ('cid', ctypes.c_ushort),
        ('bdaddr_type', ctypes.c_ubyte),
    ]


def sockaddr(address, kind, psm):
    raw = bytes(int(part, 16) for part in reversed(address.split(':'))) if address else bytes(6)
    return SockaddrL2(AF_BLUETOOTH, psm, (ctypes.c_ubyte * 6)(*raw), 0, kind)


def request_bytes(method, path, body=b''):
    head = f'{method} {path} HTTP/1.1\r\nHost: vana\r\nContent-Length: {len(body)}\r\n'
    if body:
        head += 'Content-Type: application/json\r\n'
    return (head + '\r\n').encode() + body


def parse_requests(words):
    """METHOD PATH [BODY] groups; a BODY of @FILE is that file's bytes."""
    requests = []
    items = list(words)
    while items:
        method, *rest = items
        if method.upper() not in METHODS or not rest:
            raise ValueError(f'expected METHOD PATH, got {" ".join(items[:2])!r}')
        path, items = rest[0], rest[1:]
        body = b''
        if items and items[0].upper() not in METHODS:
            text, items = items[0], items[1:]
            body = Path(text[1:]).read_bytes() if text.startswith('@') else text.encode()
        requests.append((method.upper(), path, body))
    return requests


class Channel:
    """An open v2 session over a SEQPACKET socket. SDU boundaries carry no meaning either way."""

    def __init__(self, sock, link):
        self.sock = sock
        self.link = link
        self.stream = bytearray()

    @classmethod
    def handshake(cls, sock, key, nonce_p=None):
        nonce_p = nonce_p or os.urandom(NONCE_BYTES)
        sock.send(frame(nonce_p))
        channel = cls(sock, None)
        nonce_d = channel.raw_frame()
        if len(nonce_d) != NONCE_BYTES:
            raise ConnectionError(f'device answered the hello with {len(nonce_d)} bytes, not {NONCE_BYTES}')
        channel.link = Link.derive(key, nonce_p, nonce_d)
        return channel

    def raw_frame(self):
        while (taken := take_frame(self.stream)) is None:
            try:
                part = self.sock.recv(4096)
            except ConnectionResetError:
                part = b''
            if not part:
                raise ConnectionError('device closed the channel')
            self.stream += part
        payload, rest = taken
        self.stream = bytearray(rest)
        return payload

    def send(self, plaintext, piece):
        """Seal `plaintext` into frames of at most PHONE_FRAME bytes and write them as SDUs; the SDU count."""
        step = PHONE_FRAME - TAG_BYTES
        wire = b''.join(self.link.send.seal(plaintext[at : at + step]) for at in range(0, len(plaintext), step))
        for at in range(0, len(wire), piece):
            self.sock.send(wire[at : at + piece])
        return -(-len(wire) // piece)

    def response(self):
        """Plaintext frames up to the empty one that ends a response."""
        response = bytearray()
        while plaintext := self.link.receive.open(self.raw_frame()):
            response += plaintext
        return bytes(response)

    def exchange(self, request, piece):
        sdus = self.send(request, piece)
        started = time.monotonic()
        response = self.response()
        if self.stream:
            raise AssertionError(f'{len(self.stream)} bytes after the end frame')
        return response, sdus, time.monotonic() - started


def split_response(response):
    head, _, body = response.partition(b'\r\n\r\n')
    lines = head.decode('latin-1').split('\r\n')
    return lines, body


def bluez():
    import dbus

    bus = dbus.SystemBus()
    return dbus, bus, dbus.Interface(bus.get_object('org.bluez', '/'), 'org.freedesktop.DBus.ObjectManager')


def sync_devices(manager):
    for path, interfaces in manager.GetManagedObjects().items():
        device = interfaces.get('org.bluez.Device1')
        if device and SYNC_SERVICE in [str(uuid).lower() for uuid in device.get('UUIDs', [])]:
            yield path, device


def service_id(device):
    """The id bytes in a device's sync service data, or None when BlueZ has seen none."""
    for uuid, value in device.get('ServiceData', {}).items():
        if str(uuid).lower() == SYNC_SERVICE:
            return bytes(int(byte) for byte in value)
    return None


def choose(devices, device_id):
    """(device matching the pairing id, first device without service data); either may be None."""
    match = fallback = None
    for device in devices:
        # Advertising now, or still linked to this machine.
        if 'RSSI' not in device and not bool(device.get('Connected', False)):
            continue
        seen = service_id(device)
        if seen == device_id and match is None:
            match = device
        elif seen is None and fallback is None:
            fallback = device
    return match, fallback


def discover(seconds, device_id):
    """(address, address type, how it was found) of the device advertising `device_id`."""
    dbus, bus, manager = bluez()
    objects = manager.GetManagedObjects()
    adapters = [path for path, interfaces in objects.items() if 'org.bluez.Adapter1' in interfaces]
    if not adapters:
        raise SystemExit('no Bluetooth adapter on this machine')
    adapter = dbus.Interface(bus.get_object('org.bluez', adapters[0]), 'org.bluez.Adapter1')
    adapter.SetDiscoveryFilter(dbus.Dictionary({'Transport': 'le', 'UUIDs': [SYNC_SERVICE]}, signature='sv'))
    adapter.StartDiscovery()
    try:
        deadline = time.monotonic() + seconds
        fallback_since = None
        while time.monotonic() < deadline:
            match, fallback = choose((device for _, device in sync_devices(manager)), device_id)
            if match is not None:
                return str(match['Address']), str(match.get('AddressType', 'random')), 'id'
            if fallback is not None:
                fallback_since = fallback_since or time.monotonic()
                if time.monotonic() - fallback_since >= UUID_FALLBACK_S:
                    return (
                        str(fallback['Address']),
                        str(fallback.get('AddressType', 'random')),
                        'uuid (no service data seen)',
                    )
            time.sleep(0.2)
    finally:
        adapter.StopDiscovery()
    raise SystemExit(f'no device advertises pairing id {device_id.hex()}; is it on, paired and not recording?')


def leave(address):
    """Drop the LE link to `address`, as a phone that leaves does."""
    dbus, bus, manager = bluez()
    for path, device in sync_devices(manager):
        if str(device['Address']) == address and bool(device.get('Connected', False)):
            dbus.Interface(bus.get_object('org.bluez', path), 'org.bluez.Device1').Disconnect()
            return True
    return False


def adapters():
    try:
        names = sorted(name for name in os.listdir('/sys/class/bluetooth') if name.startswith('hci'))
    except FileNotFoundError:
        names = []
    print(json.dumps({'adapters': names}))


def connect(address, kind, mtu):
    libc = ctypes.CDLL(None, use_errno=True)
    sock = socket.socket(AF_BLUETOOTH, socket.SOCK_SEQPACKET, BTPROTO_L2CAP)
    local = sockaddr(None, BDADDR_LE_PUBLIC, 0)
    if libc.bind(sock.fileno(), ctypes.byref(local), ctypes.sizeof(local)) != 0:
        raise OSError(ctypes.get_errno(), 'bind')
    sock.setsockopt(SOL_BLUETOOTH, BT_RCVMTU, struct.pack('<H', mtu))
    peer = sockaddr(address, BDADDR_LE_RANDOM if kind == 'random' else BDADDR_LE_PUBLIC, PSM)
    sock.settimeout(None)
    if libc.connect(sock.fileno(), ctypes.byref(peer), ctypes.sizeof(peer)) != 0:
        raise OSError(ctypes.get_errno(), f'connect {address} psm {PSM:#06x}')
    return sock


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('requests', nargs='*', help='METHOD PATH [BODY|@FILE] groups, sent in order over one channel')
    parser.add_argument('--pairing', help="vanaride://pair URL or a file holding one; cycling pair's file by default")
    parser.add_argument('--wrong-key', action='store_true', help='replace the key with random bytes (refusal proof)')
    parser.add_argument('--address', help='device address; found by its pairing id when left out')
    parser.add_argument('--address-type', default='random', choices=['random', 'public'])
    parser.add_argument('--piece', type=int, default=48, help='bytes per request SDU (splits frames on purpose)')
    parser.add_argument('--mtu', type=int, default=512, help='SDU size this side accepts')
    parser.add_argument('--timeout', type=float, default=20)
    parser.add_argument('--body', action='store_true', help='print each response body in full')
    parser.add_argument('--keep', action='store_true', help='keep the link afterwards')
    parser.add_argument('--adapters', action='store_true', help="only list this machine's Bluetooth adapters")
    args = parser.parse_args()
    if args.adapters:
        adapters()
        return 0
    try:
        requests = parse_requests(args.requests)
        pairing = load_pairing(args.pairing)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    if not requests:
        parser.error('give METHOD PATH pairs, such as GET /api/status')
    if args.wrong_key:
        pairing = Pairing(pairing.id, os.urandom(len(pairing.key)))
    if args.address:
        address, kind, found = args.address, args.address_type, 'address'
    else:
        address, kind, found = discover(15, pairing.id)
    sock = connect(address, kind, args.mtu)
    code = 0
    try:
        sock.settimeout(args.timeout)
        report = {'device': address, 'found_by': found, 'id': pairing.id_hex, 'wrong_key': args.wrong_key}
        started = time.monotonic()
        try:
            channel = Channel.handshake(sock, pairing.key)
        except (ConnectionError, TimeoutError) as error:
            print(json.dumps({**report, 'error': f'handshake: {error}'}))
            return 1
        print(json.dumps({**report, 'handshake_ms': round((time.monotonic() - started) * 1000)}))
        code = run_requests(channel, requests, args)
    finally:
        sock.close()
        if not args.keep:
            time.sleep(0.5)
            print(json.dumps({'left': leave(address)}))
    return code


def run_requests(channel, requests, args):
    for method, path, body in requests:
        report = {'request': f'{method} {path}'}
        if body:
            report['request_body_bytes'] = len(body)
        try:
            response, sdus, seconds = channel.exchange(request_bytes(method, path, body), args.piece)
        except AuthenticationError as error:
            print(json.dumps({**report, 'error': f'response {error}', 'counters': counters(channel)}))
            return 1
        except (ConnectionError, TimeoutError) as error:
            print(json.dumps({**report, 'error': str(error), 'counters': counters(channel)}))
            return 1
        lines, body = split_response(response)
        report |= {
            'request_sdus': sdus,
            'status': lines[0],
            'headers': lines[1:],
            'body_bytes': len(body),
            'ms': round(seconds * 1000),
            'counters': counters(channel),
        }
        if args.body or len(body) <= 600:
            report['body'] = body.decode('utf-8', 'replace')
        print(json.dumps(report))
    return 0


def counters(channel):
    """Frames sealed so far in each direction: phone to device, device to phone."""
    return {'pd': channel.link.send.counter, 'dp': channel.link.receive.counter}


if __name__ == '__main__':
    try:
        sys.exit(main())
    except OSError as error:
        sys.exit(f'ble-http: {error}')
