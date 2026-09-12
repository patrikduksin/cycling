import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from usb import UsbConnection
from terminal import exchange, run


class TerminalTests(unittest.TestCase):
    def connection(self, status='OK'):
        connection = MagicMock(request_id=0)
        connection.__enter__.return_value = connection
        connection.__exit__.return_value = False
        connection.terminal_command.return_value = dict(type='reply', id=1, status=status, data='token=8')
        return connection

    def test_one_shot_prints_accepted_without_polling_or_replaying(self):
        connection = self.connection('ACCEPTED')
        with tempfile.TemporaryDirectory() as directory:
            output = io.StringIO()
            with patch('terminal.UsbConnection', return_value=connection), contextlib.redirect_stdout(output):
                result = run('/dev/test', Path(directory) / 'log', ['RIDE', 'START'])
            self.assertEqual(result, 0)
            self.assertEqual(json.loads(output.getvalue())['status'], 'ACCEPTED')
            connection.terminal_command.assert_called_once_with('RIDE START')
            connection.__exit__.assert_called_once()

    def test_interactive_initial_info_single_owner_and_eof_release(self):
        connection = self.connection()
        with tempfile.TemporaryDirectory() as directory:
            with (patch('terminal.UsbConnection', return_value=connection) as factory,
                  patch('builtins.input', side_effect=['STATUS', EOFError()]),
                  contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO())):
                self.assertEqual(run('/dev/test', Path(directory) / 'log', []), 0)
            factory.assert_called_once()
            self.assertEqual([call.args[0] for call in connection.terminal_command.call_args_list], ['INFO', 'STATUS'])
            connection.__exit__.assert_called_once()

    def test_interactive_parse_rejection_prints_reply_then_accepts_next_command(self):
        connection = self.connection()
        connection.terminal_command.side_effect = [
            dict(type='reply', id=1, status='OK', data='metadata'),
            dict(type='reply', id=0, status='INVALID', data=''),
            dict(type='reply', id=3, status='OK', data='ready'),
        ]
        with tempfile.TemporaryDirectory() as directory:
            output = io.StringIO()
            with (patch('terminal.UsbConnection', return_value=connection),
                  patch('builtins.input', side_effect=['BOGUS', 'STATUS', EOFError()]),
                  contextlib.redirect_stdout(output), contextlib.redirect_stderr(io.StringIO())):
                self.assertEqual(run('/dev/test', Path(directory) / 'log', []), 0)
            self.assertEqual([json.loads(line)['status'] for line in output.getvalue().splitlines()],
                             ['OK', 'INVALID', 'OK'])
            self.assertEqual(connection.terminal_command.call_count, 3)

    def test_local_line_validation_precedes_transport(self):
        connection = self.connection()
        for command in ['x' * 123, 'STATUS\nRESTART', 'é']:
            with self.assertRaises(ValueError):
                exchange(connection, command)
        connection.terminal_command.assert_not_called()

    def test_firmware_rejection_is_nonzero_and_transport_error_is_not_retried(self):
        connection = self.connection('STATE')
        with tempfile.TemporaryDirectory() as directory:
            with patch('terminal.UsbConnection', return_value=connection), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(run('/dev/test', Path(directory) / 'rejected', ['RIDE', 'START']), 1)
                connection.terminal_command.side_effect = TimeoutError('uncertain')
                with self.assertRaises(TimeoutError):
                    run('/dev/test', Path(directory) / 'timeout', ['SAVE'])
            self.assertEqual(connection.terminal_command.call_count, 2)

    def test_shared_connection_cleanup_is_idempotent_even_after_close_failure(self):
        for error in [None, OSError('close failed')]:
            connection = UsbConnection('/dev/test', 'unused')
            connection.fd = 123
            log, lock = connection.log, connection.lock = MagicMock(), MagicMock()
            with patch('usb.os.close', side_effect=error) as close:
                if error:
                    with self.assertRaises(OSError):
                        connection.__exit__()
                else:
                    connection.__exit__()
                connection.__exit__()
                close.assert_called_once_with(123)
            log.close.assert_called_once()
            lock.close.assert_called_once()


if __name__ == '__main__':
    unittest.main()
