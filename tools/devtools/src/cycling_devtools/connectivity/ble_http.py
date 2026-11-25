#!/usr/bin/python3
"""Send HTTP requests to the C606 over its Bluetooth sync channel, as the phone does.

Open `PHONE START BLE` on the device first. The tool finds the device by its
sync service (or takes --address), opens an LE L2CAP channel on PSM 0x0081,
writes each request split into small SDUs and prints each framed response:
`u16 LE length` frames, a zero-length frame ending one response. Every request
goes over the same channel, one at a time. Afterwards it drops the link, as a
phone that leaves does, which ends the device's Bluetooth session (--keep
keeps it).

    cycling ble-http GET /api/status GET /api/activities GET /api/workouts
"""

import argparse
import ctypes
import json
import os
import socket
import struct
import sys
import time

SYNC_SERVICE = '56414e41-0001-4c69-6e6b-c606c606c606'
PSM = 0x0081
METHODS = ('GET', 'HEAD', 'PUT', 'POST', 'DELETE', 'OPTIONS')

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
    head = f'{method} {path} HTTP/1.1\r\nHost: vana\r\nContent-Length: {len(body)}\r\n\r\n'
    return head.encode() + body


def take_response(stream):
    """One framed response from the front of `stream`: (response, rest), or None while incomplete."""
    response = bytearray()
    at = 0
    while True:
        if len(stream) < at + 2:
            return None
        (length,) = struct.unpack_from('<H', stream, at)
        if length == 0:
            return bytes(response), bytes(stream[at + 2 :])
        if len(stream) < at + 2 + length:
            return None
        response += stream[at + 2 : at + 2 + length]
        at += 2 + length


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


def discover(seconds):
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
        while time.monotonic() < deadline:
            for _, device in sync_devices(manager):
                # Advertising now, or still linked to this machine.
                if 'RSSI' in device or bool(device.get('Connected', False)):
                    return str(device['Address']), str(device.get('AddressType', 'random'))
            time.sleep(0.2)
    finally:
        adapter.StopDiscovery()
    raise SystemExit('no device advertises the sync service; open PHONE START BLE first')


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


def exchange(sock, request, piece, timeout):
    sent = 0
    sdus = 0
    while sent < len(request):
        sock.send(request[sent : sent + piece])
        sent += piece
        sdus += 1
    stream = bytearray()
    started = time.monotonic()
    sock.settimeout(timeout)
    while (taken := take_response(stream)) is None:
        part = sock.recv(4096)
        if not part:
            raise ConnectionError('channel closed mid-response')
        stream += part
    response, rest = taken
    if rest:
        raise AssertionError(f'{len(rest)} bytes after the end frame')
    return response, sdus, time.monotonic() - started


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('requests', nargs='*', help='METHOD PATH pairs, sent in order over one channel')
    parser.add_argument('--address', help='device address; found by its sync service when left out')
    parser.add_argument('--address-type', default='random', choices=['random', 'public'])
    parser.add_argument('--piece', type=int, default=48, help='bytes per request SDU (splits the head on purpose)')
    parser.add_argument('--mtu', type=int, default=512, help='SDU size this side accepts')
    parser.add_argument('--timeout', type=float, default=20)
    parser.add_argument('--body', action='store_true', help='print each response body in full')
    parser.add_argument('--keep', action='store_true', help='keep the link afterwards')
    parser.add_argument('--adapters', action='store_true', help="only list this machine's Bluetooth adapters")
    args = parser.parse_args()
    if args.adapters:
        adapters()
        return
    words = args.requests
    if not words or len(words) % 2 or any(method.upper() not in METHODS for method in words[::2]):
        parser.error('give METHOD PATH pairs, such as GET /api/status')
    if args.address:
        address, kind = args.address, args.address_type
    else:
        address, kind = discover(15)
    sock = connect(address, kind, args.mtu)
    try:
        for method, path in zip(words[::2], words[1::2], strict=True):
            response, sdus, seconds = exchange(sock, request_bytes(method.upper(), path), args.piece, args.timeout)
            lines, body = split_response(response)
            report = {
                'request': f'{method.upper()} {path}',
                'request_sdus': sdus,
                'status': lines[0],
                'headers': lines[1:],
                'body_bytes': len(body),
                'ms': round(seconds * 1000),
            }
            if args.body or len(body) <= 600:
                report['body'] = body.decode('utf-8', 'replace')
            print(json.dumps(report))
    finally:
        sock.close()
    if not args.keep:
        time.sleep(0.5)
        print(json.dumps({'left': leave(address)}))


if __name__ == '__main__':
    try:
        main()
    except OSError as error:
        sys.exit(f'ble-http: {error}')
