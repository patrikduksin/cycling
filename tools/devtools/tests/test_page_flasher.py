"""PageFlasher against a scripted device socket; the bridge and the page are not involved."""

import json
import socket
import tempfile
import threading
import unittest
from pathlib import Path

from cycling_devtools.device.flasher import Chip, FlasherError
from cycling_devtools.device.page_flasher import PageFlasher

# A made-up address.
MAC = '020000a1b2c3'


class Door:
    """Accepts connections on a unix socket and answers each request line with `answer(request)`."""

    def __init__(self, path, answer):
        self.answer, self.requests, self.connections = answer, [], 0
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(str(path))
        self.server.listen()
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while True:
            try:
                connection, _ = self.server.accept()
            except OSError:
                return
            self.connections += 1
            with connection, connection.makefile('rwb') as stream:
                for line in stream:
                    request = json.loads(line)
                    self.requests.append(request)
                    reply = self.answer(request)
                    if reply is None:
                        break
                    stream.write(json.dumps(reply).encode() + b'\n')
                    stream.flush()

    def close(self):
        self.server.close()


class PageFlasherTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.dir = Path(temporary.name)
        self.logs = self.dir / 'logs'
        self.logs.mkdir()
        self.socket = self.dir / 'device.sock'

    def door(self, answer):
        door = Door(self.socket, answer)
        self.addCleanup(door.close)
        return door, PageFlasher(self.socket, self.logs)

    def test_every_verb_is_one_request_on_one_connection(self):
        def answer(request):
            if request['op'] == 'enter':
                return {'ok': {'mac': MAC, 'secure_boot': False, 'flash_encryption': True}}
            if request['op'] == 'verify':
                return {'ok': {'matched': request['address'] == 0x10000}}
            return {'ok': {'md5': '0' * 32}}

        door, page = self.door(answer)
        self.assertEqual(page.enter(), Chip('02:00:00:a1:b2:c3', secure_boot=False, flash_encryption=True))
        page.read(0x8000, 0x1000, Path('relative.bin'))
        page.write(0x10000, self.dir / 'image.bin')
        self.assertTrue(page.verify(0x10000, self.dir / 'image.bin'))
        self.assertFalse(page.verify(0x20000, self.dir / 'image.bin'))
        self.assertEqual(
            door.requests[1:3],
            [
                {'op': 'read', 'address': 0x8000, 'size': 0x1000, 'path': str(Path('relative.bin').resolve())},
                {'op': 'write', 'address': 0x10000, 'path': str(self.dir / 'image.bin')},
            ],
        )
        self.assertEqual(door.connections, 1)

    def test_reboot_returns_the_capture_the_agent_wrote(self):
        def answer(request):
            Path(request['path']).write_bytes(b'ESP-ROM:esp32s3\r\nboot ok\r\n')
            return {'ok': {'captured': True}}

        door, page = self.door(answer)
        self.assertEqual(page.reboot(3), b'ESP-ROM:esp32s3\r\nboot ok\r\n')
        self.assertEqual(door.requests[0]['seconds'], 3)
        self.assertFalse(Path(door.requests[0]['path']).exists())

    def test_reboot_without_the_console_coming_back_is_none(self):
        _, page = self.door(lambda request: {'ok': {'captured': False}})
        self.assertIsNone(page.reboot(3))

    def test_an_error_reply_raises_with_the_pages_reason(self):
        _, page = self.door(lambda request: {'error': {'reason': 'digestMismatch', 'detail': 'piece at 0x100000'}})
        with self.assertRaises(FlasherError) as raised:
            page.read(0, 16, self.dir / 'out.bin')
        self.assertEqual(str(raised.exception), 'read through the page failed, digestMismatch: piece at 0x100000')

    def test_a_closed_socket_or_no_bridge_is_a_flasher_error(self):
        _, page = self.door(lambda request: None)
        with self.assertRaisesRegex(FlasherError, 'closed the device socket'):
            page.enter()
        with self.assertRaisesRegex(FlasherError, 'not answering'):
            PageFlasher(self.dir / 'missing.sock', self.logs).enter()


if __name__ == '__main__':
    unittest.main()
