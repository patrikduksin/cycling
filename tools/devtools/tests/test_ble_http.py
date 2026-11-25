import struct
import unittest

from cycling_devtools.connectivity.ble_http import request_bytes, take_response


def frame(data):
    return struct.pack('<H', len(data)) + data


class BleHttpTest(unittest.TestCase):
    def test_a_response_is_its_frames_up_to_the_zero_frame(self):
        stream = frame(b'HTTP/1.1 200 OK\r\n') + frame(b'\r\n{}') + frame(b'') + frame(b'next')
        self.assertEqual(take_response(stream), (b'HTTP/1.1 200 OK\r\n\r\n{}', frame(b'next')))

    def test_a_response_without_its_zero_frame_is_incomplete(self):
        self.assertIsNone(take_response(frame(b'HTTP/1.1 200 OK\r\n')))
        self.assertIsNone(take_response(frame(b'HTTP')[:-1]))
        self.assertIsNone(take_response(b'\x00'))

    def test_a_request_declares_its_body(self):
        self.assertEqual(
            request_bytes('PUT', '/api/time', b'12'),
            b'PUT /api/time HTTP/1.1\r\nHost: vana\r\nContent-Length: 2\r\n\r\n12',
        )
