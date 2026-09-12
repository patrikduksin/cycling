import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from clear_rides import clear, mutate_once, verified_manifest


class ClearRidesTests(unittest.TestCase):
    def fixture(self, root, raw=b'x' * 512):
        (root / 'ride-slots.bin').write_bytes(raw)
        manifest = {'raw_file': 'ride-slots.bin', 'upper_bound': len(raw) // 256,
                    'slot_size': 256, 'raw_sha256': hashlib.sha256(raw).hexdigest()}
        (root / 'manifest.json').write_text(json.dumps(manifest))
        return manifest

    def test_requires_matching_complete_export(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.fixture(root)
            self.assertEqual(verified_manifest(root)['upper_bound'], 2)
            manifest['raw_sha256'] = '0' * 64
            (root / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, 'digest'):
                verified_manifest(root)

    def run_clear(self, export, output, raw, infos, replies=None):
        connection = MagicMock()
        context = MagicMock()
        context.__enter__.return_value = connection
        context.__exit__.return_value = False
        connection.terminal_command.side_effect = replies or [
            {'status': 'ACCEPTED', 'data': 'token=7'},
            {'status': 'OK', 'data': 'state=clearing slot=2 pending=7'},
            {'status': 'OK', 'data': 'state=ready slot=0 pending=0 completed=7 result=OK'},
        ]
        with (patch('clear_rides.UsbConnection', return_value=context) as factory,
              patch('clear_rides.info', side_effect=infos),
              patch('clear_rides.read_slot', side_effect=[raw[i:i + 256]
                                                          for i in range(0, len(raw), 256)]),
              patch('clear_rides.time.sleep')):
            self.connection, self.factory, self.context = connection, factory, context
            return clear('/dev/test', export, output)

    def test_current_prefix_and_info_must_match_before_command(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            export, output = root / 'export', root / 'out'
            export.mkdir()
            raw = b'x' * 512
            self.fixture(export, raw)
            info = {'version': 1, 'slot_size': 256, 'upper_bound': 2, 'status': 'ready'}
            with self.assertRaisesRegex(RuntimeError, 'does not match'):
                self.run_clear(export, output, b'y' * 512, [info, info])
            self.connection.terminal_command.assert_not_called()
            with self.assertRaisesRegex(RuntimeError, 'advanced'):
                self.run_clear(export, root / 'out2', raw, [info, {**info, 'upper_bound': 3}])
            self.connection.terminal_command.assert_not_called()

    def test_one_serial_owner_and_one_mutation_then_status_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            export = root / 'export'
            export.mkdir()
            raw = b'x' * 512
            self.fixture(export, raw)
            info = {'version': 1, 'slot_size': 256, 'upper_bound': 2, 'status': 'ready'}
            self.run_clear(export, root / 'ok', raw, [info, info, {**info, 'upper_bound': 0}])
            self.factory.assert_called_once()
            self.context.__enter__.assert_called_once()
            self.context.__exit__.assert_called_once()
            commands = [call.args[0] for call in self.connection.terminal_command.call_args_list]
            self.assertEqual(commands, ['RIDE CLEAR CONFIRM 2', 'RIDE STATUS', 'RIDE STATUS'])
            for replies in [[TimeoutError()], [{'status': 'ACCEPTED', 'data': 'token=7'}, TimeoutError()]]:
                with self.assertRaisesRegex(RuntimeError, 'uncertain'):
                    self.run_clear(export, root / f'timeout{len(replies)}', raw, [info, info], replies)
                commands = [call.args[0] for call in self.connection.terminal_command.call_args_list]
                self.assertEqual(commands.count('RIDE CLEAR CONFIRM 2'), 1)
                self.assertTrue(all(command == 'RIDE STATUS' for command in commands[1:]))

    def test_rejected_failed_missing_and_wrong_completion_are_not_retried(self):
        for replies in [
            [{'status': 'STATE', 'data': ''}],
            [{'status': 'ACCEPTED', 'data': 'token=1'}, {'status': 'OK', 'data': 'completed=1 result=FAILED'}],
            [{'status': 'ACCEPTED', 'data': 'token=1'}, {'status': 'OK', 'data': 'pending=0'}],
            [{'status': 'ACCEPTED', 'data': 'token=1'}, {'status': 'OK', 'data': 'completed=2 result=OK'}],
            [{'status': 'ACCEPTED', 'data': 'token=0'}],
        ]:
            connection = MagicMock()
            connection.terminal_command.side_effect = replies
            with self.assertRaises(RuntimeError):
                mutate_once(connection, 'RIDE INIT')
            commands = [call.args[0] for call in connection.terminal_command.call_args_list]
            self.assertEqual(commands.count('RIDE INIT'), 1)

    def test_completion_deadline_never_reissues_mutation(self):
        connection = MagicMock()
        connection.terminal_command.return_value = {'status': 'ACCEPTED', 'data': 'token=1'}
        with patch('clear_rides.time.monotonic', side_effect=[0, 91]):
            with self.assertRaises(TimeoutError):
                mutate_once(connection, 'RIDE INIT')
        connection.terminal_command.assert_called_once()


if __name__ == '__main__':
    unittest.main()
