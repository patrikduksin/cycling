import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from clear_rides import clear, verified_manifest


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

    def run_clear(self, export, output, raw, infos, command_result=None, command_error=None):
        connection = MagicMock()
        context = MagicMock()
        context.__enter__.return_value = connection
        context.__exit__.return_value = False
        device = MagicMock(active=True)
        device.__enter__.return_value = device
        device.__exit__.return_value = False
        if command_error:
            device.command.side_effect = command_error
        else:
            device.command.return_value = command_result or {
                'ride_recording': 'ready', 'recording_slot': 0
            }
        with (patch('clear_rides.ExportConnection', return_value=context),
              patch('clear_rides.info', side_effect=infos),
              patch('clear_rides.read_slot', side_effect=[raw[i:i + 256]
                                                          for i in range(0, len(raw), 256)]),
              patch('clear_rides.Device', return_value=device) as factory):
            try:
                result = clear('/dev/test', export, output)
            except BaseException:
                result = None
                raise
            finally:
                self.factory, self.device = factory, device
        return result

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
            self.factory.assert_not_called()
            with self.assertRaisesRegex(RuntimeError, 'advanced'):
                self.run_clear(export, root / 'out2', raw, [info, {**info, 'upper_bound': 3}])
            self.factory.assert_not_called()

    def test_valid_clear_is_sent_once_inactive_and_timeout_is_not_retried(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            export = root / 'export'
            export.mkdir()
            raw = b'x' * 512
            self.fixture(export, raw)
            info = {'version': 1, 'slot_size': 256, 'upper_bound': 2, 'status': 'ready'}
            self.run_clear(export, root / 'ok', raw, [info, info])
            self.device.command.assert_called_once_with('RIDE CLEAR CONFIRM 2', timeout=90)
            self.assertFalse(self.device.active)
            with self.assertRaisesRegex(RuntimeError, 'uncertain'):
                self.run_clear(export, root / 'timeout', raw, [info, info],
                               command_error=TimeoutError())
            self.device.command.assert_called_once()


if __name__ == '__main__':
    unittest.main()
