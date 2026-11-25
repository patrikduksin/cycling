import contextlib
import io
import json
import os
import socket
import threading
import unittest
from argparse import Namespace

from cycling_devtools.connectivity.ble_http import (
    PHONE_FRAME,
    SYNC_SERVICE,
    Channel,
    choose,
    parse_requests,
    request_bytes,
    run_requests,
)
from cycling_devtools.connectivity.vana_link import AuthenticationError, Link, frame, take_frame

KEY = bytes(range(32))


class FakeDevice:
    """The device side of protocol v2 over a SEQPACKET socket, as the firmware runs it."""

    def __init__(self, sock, key=KEY, tamper=False):
        self.sock, self.key, self.tamper = sock, key, tamper
        self.stream = b''
        self.requests = []
        self.frame_sizes = []
        self.closed_on_bad_frame = False
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def raw_frame(self):
        while (taken := take_frame(self.stream)) is None:
            part = self.sock.recv(4096)
            if not part:
                raise EOFError
            self.stream += part
        payload, self.stream = taken
        return payload

    def serve(self):
        try:
            nonce_p = self.raw_frame()
            nonce_d = os.urandom(16)
            self.sock.send(frame(nonce_d))
            link = Link.derive(self.key, nonce_p, nonce_d, phone=False)
            while True:
                request = b''
                while not complete(request):
                    sealed = self.raw_frame()
                    self.frame_sizes.append(len(sealed))
                    request += link.receive.open(sealed)
                self.requests.append(request)
                body = b'{"path":"%s"}' % request.split(b' ')[1] + b'.' * 300
                response = b'HTTP/1.1 200 OK\r\nContent-Length: %d\r\n\r\n' % len(body) + body
                wire = b''.join(link.send.seal(response[at : at + 100]) for at in range(0, len(response), 100))
                wire += link.send.seal(b'')
                if self.tamper:
                    wire = wire[:5] + bytes([wire[5] ^ 1]) + wire[6:]
                for at in range(0, len(wire), 7):
                    self.sock.send(wire[at : at + 7])
        except AuthenticationError:
            self.closed_on_bad_frame = True
            self.sock.close()
        except EOFError, ConnectionResetError:
            pass


def complete(request):
    head, separator, body = request.partition(b'\r\n\r\n')
    if not separator:
        return False
    length = next(
        (int(line.split(b':')[1]) for line in head.split(b'\r\n') if line.lower().startswith(b'content-length:')), 0
    )
    return len(body) >= length


class ChannelTest(unittest.TestCase):
    def pair(self, **device):
        phone, other = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        phone.settimeout(5)
        self.addCleanup(other.close)
        self.addCleanup(phone.close)
        return phone, FakeDevice(other, **device)

    def test_requests_and_responses_cross_the_sealed_channel(self):
        phone, device = self.pair()
        channel = Channel.handshake(phone, KEY)
        put = request_bytes('PUT', '/api/wifi', b'{"ssid":"nope-xyz","password":"12345678"}')
        for request in (request_bytes('GET', '/api/status'), put):
            response, sdus, _ = channel.exchange(request, 48)
            self.assertGreater(sdus, 1)
            self.assertTrue(response.startswith(b'HTTP/1.1 200 OK'))
            self.assertIn(b'.' * 300, response)
        self.assertEqual(device.requests, [request_bytes('GET', '/api/status'), put])
        self.assertEqual((channel.link.send.counter, channel.link.receive.counter), (2, 10))

    def test_a_long_request_spans_frames_of_at_most_1024_bytes(self):
        phone, device = self.pair()
        channel = Channel.handshake(phone, KEY)
        request = request_bytes('PUT', '/api/workouts', b'x' * 2500)
        channel.exchange(request, 48)
        self.assertEqual(device.frame_sizes, [PHONE_FRAME, PHONE_FRAME, len(request) - 2 * (PHONE_FRAME - 16) + 16])

    def test_a_wrong_key_makes_the_device_close_the_channel(self):
        phone, device = self.pair()
        channel = Channel.handshake(phone, os.urandom(32))
        with self.assertRaisesRegex(ConnectionError, 'closed'):
            channel.exchange(request_bytes('GET', '/api/status'), 48)
        device.thread.join(5)
        self.assertTrue(device.closed_on_bad_frame)

    def test_a_changed_response_byte_fails_to_authenticate(self):
        phone, _ = self.pair(tamper=True)
        channel = Channel.handshake(phone, KEY)
        with self.assertRaises(AuthenticationError):
            channel.exchange(request_bytes('GET', '/api/status'), 48)

    def test_a_refused_request_is_a_json_error_and_a_failure(self):
        phone, _ = self.pair()
        channel = Channel.handshake(phone, os.urandom(32))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = run_requests(channel, [('GET', '/api/status', b'')], Namespace(piece=48, body=False))
        report = json.loads(out.getvalue())
        self.assertEqual(code, 1)
        self.assertEqual(report['error'], 'device closed the channel')
        self.assertEqual(report['counters'], {'pd': 1, 'dp': 0})


class RequestTest(unittest.TestCase):
    def test_a_request_declares_its_body(self):
        self.assertEqual(
            request_bytes('PUT', '/api/time', b'12'),
            b'PUT /api/time HTTP/1.1\r\nHost: vana\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\n12',
        )

    def test_a_body_follows_its_method_and_path(self):
        self.assertEqual(
            parse_requests(['post', '/api/wifi/scan', 'GET', '/api/wifi', 'PUT', '/api/wifi', '{"ssid":"x"}']),
            [('POST', '/api/wifi/scan', b''), ('GET', '/api/wifi', b''), ('PUT', '/api/wifi', b'{"ssid":"x"}')],
        )
        with self.assertRaises(ValueError):
            parse_requests(['GET'])
        with self.assertRaises(ValueError):
            parse_requests(['/api/status'])


class DiscoveryTest(unittest.TestCase):
    def test_the_device_with_the_pairing_id_wins(self):
        def device(address, data=None, rssi=True):
            found = {'Address': address, 'UUIDs': [SYNC_SERVICE]}
            if data is not None:
                found['ServiceData'] = {SYNC_SERVICE.upper(): list(data)}
            if rssi:
                found['RSSI'] = -60
            return found

        mine = bytes.fromhex('0a1b2c3d')
        devices = [
            device('A', b'\x01\x02\x03\x04'),
            device('B'),
            device('C', mine, rssi=False),
            device('D', mine),
        ]
        match, fallback = choose(devices, mine)
        self.assertEqual((match['Address'], fallback['Address']), ('D', 'B'))
        self.assertEqual(choose(devices[:1], mine), (None, None))


if __name__ == '__main__':
    unittest.main()
