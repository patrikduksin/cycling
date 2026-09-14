import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from usb import UsbConnection, acquire_lock


class UsbTests(unittest.TestCase):
    def test_lock_excludes_other_owners_and_releases_after_close(self):
        with tempfile.TemporaryDirectory() as directory, patch('usb.ROOT', Path(directory)):
            with acquire_lock():
                with self.assertRaises(BlockingIOError):
                    acquire_lock()
            with acquire_lock():
                pass

    def test_disconnect_or_reboot_never_replays_pending_mutation(self):
        for data in (b'', b'CYCLING_BOOT version=test\n'):
            connection = UsbConnection('/dev/test', 'unused')
            connection.fd, connection.log = 123, io.BytesIO()
            with (patch('usb.os.write', return_value=len(b'CMD 1 SAVE\n')) as write,
                  patch('usb.select.select', return_value=([123], [], [])),
                  patch('usb.os.read', return_value=data)):
                with self.assertRaises(RuntimeError):
                    connection.terminal_command('SAVE')
                write.assert_called_once_with(123, b'CMD 1 SAVE\n')

    def test_secret_bearing_timeout_never_includes_plaintext_or_hex_and_never_replays(self):
        password = 'private-test-password'
        command = f'WIFI CONFIG WPA2 54657374 {password.encode().hex()}'
        connection = UsbConnection('/dev/test', 'unused')
        connection.fd, connection.log = 123, io.BytesIO()
        request = f'CMD 1 {command}\n'.encode()
        with patch('usb.os.write', return_value=len(request)) as write:
            with self.assertRaises(TimeoutError) as raised:
                connection.terminal_command(command, timeout=0)
            write.assert_called_once_with(123, request)
        self.assertNotIn(password, str(raised.exception))
        self.assertNotIn(password.encode().hex(), str(raised.exception))
        self.assertNotIn(command, str(raised.exception))


if __name__ == '__main__':
    unittest.main()
