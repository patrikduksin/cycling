import hashlib
import json
import tempfile
import tracemalloc
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from cycling_devtools.rides.clear import clear, mutate_once, verified_manifest


class ClearRidesTests(unittest.TestCase):
    def fixture(self, root, raw=b'x' * 512):
        (root / 'ride-slots.bin').write_bytes(raw)
        manifest = {
            'raw_file': 'ride-slots.bin',
            'upper_bound': len(raw) // 256,
            'slot_size': 256,
            'raw_sha256': hashlib.sha256(raw).hexdigest(),
        }
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

    def test_a_bulk_sized_export_is_verified_in_bounded_memory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / 'ride-slots.bin'
            block = bytes(range(256)) * 4096
            digest = hashlib.sha256()
            with raw.open('wb') as out:
                for _ in range(32):
                    out.write(block)
                    digest.update(block)
            slots = raw.stat().st_size // 256
            manifest = {
                'raw_file': raw.name,
                'upper_bound': slots,
                'slot_size': 256,
                'raw_sha256': digest.hexdigest(),
                'medium': 'bulk',
                'capacity': 5_777_391,
                'invalid_containers': [],
                'invalid_sectors': [],
            }
            (root / 'manifest.json').write_text(json.dumps(manifest))
            tracemalloc.start()
            try:
                self.assertEqual(verified_manifest(root)['upper_bound'], slots)
                _, peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            # 32 MiB of slots; reading the file whole needed more than that.
            self.assertLess(peak, 4 * 2**20, f'{peak / 2**20:.1f} MiB peak')
            # A file of the wrong length is refused from its size, before any read.
            with raw.open('r+b') as out:
                out.truncate(4 * 2**30)
            tracemalloc.start()
            try:
                with self.assertRaisesRegex(ValueError, 'length'):
                    verified_manifest(root)
                _, peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            self.assertLess(peak, 4 * 2**20)

    def run_clear(
        self, export, output, raw, infos, replies=None, medium='internal', invalid=(), sectors=None, data_start=16
    ):
        connection = MagicMock()
        context = MagicMock()
        context.__enter__.return_value = connection
        context.__exit__.return_value = False
        connection.terminal_command.side_effect = replies or [
            {'status': 'ACCEPTED', 'data': 'token=7'},
            {'status': 'OK', 'data': 'state=clearing slot=2 pending=7'},
            {'status': 'OK', 'data': 'state=ready slot=0 pending=0 completed=7 result=OK'},
        ]
        with (
            patch('cycling_devtools.rides.clear.UsbConnection', return_value=context) as factory,
            patch('cycling_devtools.rides.clear.info', side_effect=infos),
            patch(
                'cycling_devtools.rides.clear.read_slot',
                side_effect=[
                    (raw[i : i + 256], i // 256 not in invalid) if medium == 'bulk' else raw[i : i + 256]
                    for i in range(0, len(raw), 256)
                ],
            ),
            patch('cycling_devtools.rides.clear.read_owned_sector', side_effect=lambda _, sector: sectors[sector]),
            patch('cycling_devtools.rides.clear.bulk_data_start', return_value=data_start),
            patch('cycling_devtools.rides.clear.time.sleep'),
        ):
            self.connection, self.factory, self.context = connection, factory, context
            return clear('/dev/test', export, output, medium)

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
            self.assertEqual(commands, ['RIDE CLEAR CONFIRM 2', 'RIDE INTERNAL STATUS', 'RIDE INTERNAL STATUS'])
            for replies in [[TimeoutError()], [{'status': 'ACCEPTED', 'data': 'token=7'}, TimeoutError()]]:
                with self.assertRaisesRegex(RuntimeError, 'uncertain'):
                    self.run_clear(export, root / f'timeout{len(replies)}', raw, [info, info], replies)
                commands = [call.args[0] for call in self.connection.terminal_command.call_args_list]
                self.assertEqual(commands.count('RIDE CLEAR CONFIRM 2'), 1)
                self.assertTrue(all(command == 'RIDE INTERNAL STATUS' for command in commands[1:]))

    def bulk_fixture(self, root, raw):
        manifest = self.fixture(root, raw)
        manifest.update(
            medium='bulk',
            store_id='00000000000000ab',
            generation=1,
            capacity=5_777_391,
            invalid_containers=[],
            invalid_sectors=[],
        )
        (root / 'manifest.json').write_text(json.dumps(manifest))
        return manifest

    def test_bulk_clear_verifies_the_raw_sectors_behind_invalid_containers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            export = root / 'export'
            export.mkdir()
            raw = b'x' * 256 + bytes(256)
            manifest = self.bulk_fixture(export, raw)
            kept = b'\xa5' * 512
            (export / 'invalid-sector-17.bin').write_bytes(kept)
            manifest.update(
                invalid_containers=[1],
                invalid_sectors=[
                    {
                        'index': 1,
                        'sector': 17,
                        'file': 'invalid-sector-17.bin',
                        'sha256': hashlib.sha256(kept).hexdigest(),
                    }
                ],
            )
            (export / 'manifest.json').write_text(json.dumps(manifest))
            info = {
                'version': 1,
                'slot_size': 256,
                'upper_bound': 2,
                'status': 'ready',
                'medium': 'bulk',
                'store_id': '00000000000000ab',
                'generation': 1,
                'capacity': 5_777_391,
            }
            # Records that moved (a new volume table) are not the exported ones.
            with self.assertRaisesRegex(RuntimeError, 'moved since the export'):
                self.run_clear(export, root / 'moved', raw, [info, info], medium='bulk', invalid={1}, data_start=2064)
            self.connection.terminal_command.assert_not_called()
            # The device sector changed after export: no clear is sent.
            with self.assertRaisesRegex(RuntimeError, 'invalid sectors do not match'):
                self.run_clear(
                    export, root / 'changed', raw, [info, info], medium='bulk', invalid={1}, sectors={17: b'\0' * 512}
                )
            self.connection.terminal_command.assert_not_called()
            # A container that became valid, or a new invalid one, also differs.
            with self.assertRaisesRegex(RuntimeError, 'invalid sectors do not match'):
                self.run_clear(export, root / 'valid', raw, [info, info], medium='bulk', sectors={17: kept})
            self.connection.terminal_command.assert_not_called()
            # Matching raw sectors clear.
            self.run_clear(
                export,
                root / 'ok',
                raw,
                [info, info, {**info, 'upper_bound': 0}],
                medium='bulk',
                invalid={1},
                sectors={17: kept},
            )
            commands = [call.args[0] for call in self.connection.terminal_command.call_args_list]
            self.assertEqual(commands[0], 'RIDE BULK CLEAR CONFIRM 2')
            # A deleted or edited raw sector file fails verification before any device access.
            (export / 'invalid-sector-17.bin').write_bytes(b'edited')
            with self.assertRaisesRegex(ValueError, 'does not match its manifest'):
                verified_manifest(export)
            (export / 'invalid-sector-17.bin').unlink()
            with self.assertRaisesRegex(ValueError, 'lacks raw sector 17'):
                verified_manifest(export)
            manifest.pop('invalid_sectors')
            (export / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, 'invalid container list'):
                verified_manifest(export)

    def test_bulk_clear_targets_the_bulk_log_of_the_exported_store(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            export = root / 'export'
            export.mkdir()
            raw = b'x' * 512
            self.bulk_fixture(export, raw)
            info = {
                'version': 1,
                'slot_size': 256,
                'upper_bound': 2,
                'status': 'ready',
                'medium': 'bulk',
                'store_id': '00000000000000ab',
                'generation': 1,
                'capacity': 5_777_391,
            }
            summary = self.run_clear(export, root / 'ok', raw, [info, info, {**info, 'upper_bound': 0}], medium='bulk')
            self.assertEqual(summary['medium'], 'bulk')
            commands = [call.args[0] for call in self.connection.terminal_command.call_args_list]
            self.assertEqual(commands, ['RIDE BULK CLEAR CONFIRM 2', 'RIDE BULK STATUS', 'RIDE BULK STATUS'])
            # A reformatted store is a different epoch even with equal contents.
            other = {**info, 'store_id': '00000000000000cd'}
            with self.assertRaisesRegex(RuntimeError, 'store changed'):
                self.run_clear(export, root / 'other', raw, [other, other], medium='bulk')
            self.connection.terminal_command.assert_not_called()
            # Media of different exports are never mixed up.
            with self.assertRaisesRegex(ValueError, 'not a internal export'):
                self.run_clear(export, root / 'mixed', raw, [info, info])

    def test_bulk_export_bounds_use_the_store_capacity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.bulk_fixture(root, b'x' * 512)
            manifest['capacity'] = 1
            (root / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, 'bounds'):
                verified_manifest(root)

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

    def test_busy_reads_status_once_then_retries_once(self):
        # An earlier tool's unread completion is discarded by the status read.
        connection = MagicMock()
        connection.terminal_command.side_effect = [
            {'status': 'BUSY', 'data': ''},
            {'status': 'OK', 'data': 'state=ready pending=0 completed=3 result=OK'},
            {'status': 'ACCEPTED', 'data': 'token=4'},
            {'status': 'OK', 'data': 'state=ready pending=0 completed=4 result=OK'},
        ]
        with patch('cycling_devtools.rides.clear.time.sleep'):
            self.assertEqual(mutate_once(connection, 'RIDE INIT')['completed'], '4')
        commands = [call.args[0] for call in connection.terminal_command.call_args_list]
        self.assertEqual(commands, ['RIDE INIT', 'RIDE STATUS', 'RIDE INIT', 'RIDE STATUS'])
        # An operation still in flight stays BUSY, and is not retried again.
        connection = MagicMock()
        connection.terminal_command.side_effect = [
            {'status': 'BUSY', 'data': ''},
            {'status': 'OK', 'data': 'state=recording pending=9'},
            {'status': 'BUSY', 'data': ''},
        ]
        with self.assertRaisesRegex(RuntimeError, 'rejected: BUSY'):
            mutate_once(connection, 'RIDE INIT')
        commands = [call.args[0] for call in connection.terminal_command.call_args_list]
        self.assertEqual(commands.count('RIDE INIT'), 2)

    def test_completion_deadline_never_reissues_mutation(self):
        connection = MagicMock()
        connection.terminal_command.return_value = {'status': 'ACCEPTED', 'data': 'token=1'}
        with patch('cycling_devtools.rides.clear.time.monotonic', side_effect=[0, 91]):
            with self.assertRaises(TimeoutError):
                mutate_once(connection, 'RIDE INIT')
        connection.terminal_command.assert_called_once()


if __name__ == '__main__':
    unittest.main()
