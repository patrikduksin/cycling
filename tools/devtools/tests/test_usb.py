import io
import json
import os
import select
import tempfile
import threading
import tty
import unittest
from pathlib import Path
from unittest.mock import patch

from cycling_devtools.terminal.usb import UsbConnection, acquire_lock


class Console(threading.Thread):
    """The firmware's line framing (packages/console) on a pty master: anything but `CMD <id> <verb>` in 256 bytes is rejected with id zero."""

    def __init__(self, master, held=b''):
        super().__init__(daemon=True)
        self.master, self.line, self.done = master, bytearray(held), threading.Event()

    def reply(self, id, status, data):
        os.write(self.master, json.dumps(dict(type='reply', id=id, status=status, data=data)).encode() + b'\n')

    def run(self):
        while not self.done.is_set():
            if not select.select([self.master], [], [], 0.05)[0]:
                continue
            for byte in os.read(self.master, 4096):
                if byte != ord('\n'):
                    self.line.append(byte)
                    continue
                line, self.line = bytes(self.line), bytearray()
                words = line.split()
                if len(line) > 256:
                    self.reply(0, 'INVALID', 'line exceeds 256 bytes')
                elif len(words) == 3 and words[0] == b'CMD' and words[1].isdigit():
                    self.reply(int(words[1]), 'OK', 'board=c606')
                else:
                    self.reply(0, 'INVALID', 'malformed command')


class AttachTests(unittest.TestCase):
    def attach(self, backlog=b'', held=b'', command='INFO'):
        master, slave = os.openpty()
        tty.setraw(slave)
        console = Console(master, held)
        os.write(master, backlog)
        with tempfile.TemporaryDirectory() as directory:
            port = Path(directory) / 'port'
            port.symlink_to(os.ttyname(slave))
            try:
                with (
                    patch('cycling_devtools.terminal.usb.DEVICE_LOCAL', Path(directory) / '.local'),
                    UsbConnection(port, Path(directory) / 'usb.log') as connection,
                ):
                    console.start()
                    return connection.terminal_command(command, timeout=2)
            finally:
                console.done.set()
                if console.is_alive():
                    console.join()
                os.close(slave)
                os.close(master)

    def test_first_command_ignores_what_the_port_held_before_the_attachment(self):
        # A bridged port (cycling remote) keeps device output nobody read yet, and opening the
        # visitor's port leaves a partial line in the device (17 bytes on a C606 in desktop Chrome).
        stale = b'CYCLING_BOOT version=0.1.0 board=magene-c606\n{"type":"reply","id":0,"status":"INVALID","data":"x"}\n'
        reply = self.attach(backlog=stale, held=b'CYCLING_BOOT vers')
        self.assertEqual((reply['status'], reply['data']), ('OK', 'board=c606'))

    def test_first_command_on_a_clean_port_is_answered(self):
        self.assertEqual(self.attach()['status'], 'OK')

    def test_malformed_first_command_is_still_rejected(self):
        self.assertEqual(self.attach(command='NOT A COMMAND')['status'], 'INVALID')


class UsbTests(unittest.TestCase):
    def test_reenumerated_tty_waits_before_first_accepted_submission(self):
        connection = UsbConnection('/dev/test', 'unused')
        connection.request_id = 0  # the first request is CMD 1 below
        connection.fd, connection.log = 123, io.BytesIO()
        request = b'CMD 1 SAVE\n'
        response = b'{"type":"reply","id":1,"status":"OK","data":""}\n'
        with (
            patch('cycling_devtools.terminal.usb.os.write', side_effect=[BlockingIOError(), len(request)]) as write,
            patch('cycling_devtools.terminal.usb.select.select', return_value=([123], [123], [])),
            patch('cycling_devtools.terminal.usb.os.read', return_value=response),
        ):
            self.assertEqual(connection.terminal_command('SAVE')['status'], 'OK')
            self.assertEqual(write.call_count, 2)

    def test_short_submission_is_never_replayed(self):
        connection = UsbConnection('/dev/test', 'unused')
        connection.request_id = 0  # the first request is CMD 1 below
        connection.fd, connection.log = 123, io.BytesIO()
        with patch('cycling_devtools.terminal.usb.os.write', return_value=3) as write:
            with self.assertRaisesRegex(RuntimeError, 'uncertain'):
                connection.terminal_command('SAVE')
            write.assert_called_once()

    def test_unwritable_timeout_reports_no_submission(self):
        connection = UsbConnection('/dev/test', 'unused')
        connection.request_id = 0  # the first request is CMD 1 below
        connection.fd, connection.log = 123, io.BytesIO()
        with patch('cycling_devtools.terminal.usb.os.write', side_effect=BlockingIOError()) as write:
            with self.assertRaisesRegex(TimeoutError, 'not submitted'):
                connection.terminal_command('SAVE', timeout=0)
            write.assert_called_once()

    def test_wait_expiring_cannot_submit_a_mutation_after_its_deadline(self):
        connection = UsbConnection('/dev/test', 'unused')
        connection.request_id = 0  # the first request is CMD 1 below
        connection.fd, connection.log = 123, io.BytesIO()
        with (
            patch(
                'cycling_devtools.terminal.usb.os.write', side_effect=[BlockingIOError(), len(b'CMD 1 SAVE\n')]
            ) as write,
            patch('cycling_devtools.terminal.usb.time.monotonic', side_effect=[0, 0.5, 1.1]),
            patch('cycling_devtools.terminal.usb.select.select', return_value=([], [123], [])),
        ):
            with self.assertRaisesRegex(TimeoutError, 'not submitted'):
                connection.terminal_command('SAVE', timeout=1)
            write.assert_called_once()

    def test_lock_excludes_other_owners_and_releases_after_close(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            patch('cycling_devtools.terminal.usb.DEVICE_LOCAL', Path(directory) / '.local'),
        ):
            with acquire_lock(), self.assertRaises(BlockingIOError):
                acquire_lock()
            with acquire_lock():
                pass

    def test_disconnect_or_reboot_never_replays_pending_mutation(self):
        for data in (b'', b'CYCLING_BOOT version=test\n'):
            connection = UsbConnection('/dev/test', 'unused')
            connection.request_id = 0  # the first request is CMD 1 below
            connection.fd, connection.log = 123, io.BytesIO()
            with (
                patch('cycling_devtools.terminal.usb.os.write', return_value=len(b'CMD 1 SAVE\n')) as write,
                patch('cycling_devtools.terminal.usb.select.select', return_value=([123], [], [])),
                patch('cycling_devtools.terminal.usb.os.read', return_value=data),
            ):
                with self.assertRaises(RuntimeError):
                    connection.terminal_command('SAVE')
                write.assert_called_once_with(123, b'CMD 1 SAVE\n')

    def test_secret_bearing_timeout_never_includes_plaintext_or_hex_and_never_replays(self):
        password = 'private-test-password'
        command = f'WIFI CONFIG WPA2 54657374 {password.encode().hex()}'
        connection = UsbConnection('/dev/test', 'unused')
        connection.request_id = 0  # the first request is CMD 1 below
        connection.fd, connection.log = 123, io.BytesIO()
        request = f'CMD 1 {command}\n'.encode()
        with patch('cycling_devtools.terminal.usb.os.write', return_value=len(request)) as write:
            with self.assertRaises(TimeoutError) as raised:
                connection.terminal_command(command, timeout=0)
            write.assert_called_once_with(123, request)
        self.assertNotIn(password, str(raised.exception))
        self.assertNotIn(password.encode().hex(), str(raised.exception))
        self.assertNotIn(command, str(raised.exception))


if __name__ == '__main__':
    unittest.main()
