"""Offline executor safety tests; fake Connection never opens USB."""
import json
import struct
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import mmc_partition_apply as executor

class ApplyPartitionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.baseline = self.root / 'baseline.bin'
        self.baseline.write_bytes(bytes(12 * 512))
        self.plan_path = self.root / 'plan.json'
        self.plan_path.write_text('{}')
        layout = executor.Layout(total=12, stock_start=2, stock_sectors=4,
                                 custom_start=6, custom_sectors=6)
        targets, extents = {}, []
        for phase, start, payload in [('stock', 2, b'S' * 2048),
                                      ('marker', 6, b'K' * 512), ('mbr_commit', 0, b'M' * 512)]:
            path = self.root / (phase + '.bin')
            path.write_bytes(payload)
            record = dict(phase=phase, start=start, count=len(payload) // 512,
                          path=path, sha256=executor.digest_file(path))
            targets[phase] = record
            extents.append(record)
        plan = {'baseline_sha256': executor.digest_file(self.baseline), 'changed_sectors': 6}
        self.preflight_result = (plan, layout, self.baseline, targets, extents)
        self.journal = self.root / 'execution.jsonl'
        baseline = self.baseline

        class FakeConnection:
            instances = []
            fail_write = None
            corrupt_stock_verify = False
            corrupt_stock_sample = False
            corrupt_boundary = False

            def __init__(self, port):
                self.data = bytearray(baseline.read_bytes())
                self.writes, self.arms, self.reads = [], [], []
                self.closed = False
                self.instances.append(self)

            def __enter__(self):
                return self

            def __exit__(self, *_):
                self.closed = True

            def info(self):
                return {'total_sectors': 12}

            def arm(self, start, count):
                self.arms.append((start, count))

            def write_sector(self, start, payload):
                self.writes.append(start)
                self.data[start * 512:(start + 1) * 512] = payload
                if start == self.fail_write:
                    # Model physical completion followed by a lost ACK.
                    raise TimeoutError('completion uncertain')

            def read(self, start, count):
                self.reads.append((start, count))
                data = bytes(self.data[start * 512:(start + count) * 512])
                if self.corrupt_stock_verify and (start, count) == (2, 4):
                    data = b'X' + data[1:]
                if self.corrupt_stock_sample and (start, count) == (2, 1):
                    data = b'X' + data[1:]
                if self.corrupt_boundary and self.writes and (start, count) == (1, 1):
                    data = b'X' + data[1:]
                yield data

            def recover(self):
                raise AssertionError('executor must not automatically recover')

        self.fake = FakeConnection

    def run_fake(self, **kwargs):
        with patch.object(executor, 'preflight', return_value=self.preflight_result), \
                patch.object(executor, 'stock_sample_sectors', return_value=[0, 3]), \
                patch.object(executor, 'Connection', self.fake):
            executor.apply(self.plan_path, self.journal, 'FAKE-ONLY', **kwargs)

    def events(self):
        return [json.loads(line) for line in self.journal.read_text().splitlines()]

    def test_success_verifies_entire_stock_before_marker_and_commits_mbr_last(self):
        self.run_fake()
        connection = self.fake.instances[-1]
        self.assertEqual(connection.writes, [2, 3, 4, 5, 6, 0])
        self.assertEqual(connection.arms, [(2, 4), (6, 1), (0, 1)])
        self.assertEqual(connection.reads[-6:], [(2, 4), (1, 1), (6, 1), (11, 1), (6, 1), (0, 1)])
        events = self.events()
        order = [(e['event'], e.get('phase')) for e in events]
        self.assertLess(order.index(('target_verified', 'stock')), order.index(('arm_intent', 'marker')))
        self.assertLess(order.index(('target_verified', 'marker')), order.index(('arm_intent', 'mbr_commit')))
        self.assertEqual(events[-1]['event'], 'complete')
        self.assertTrue(connection.closed)

    def test_stock_mismatch_blocks_marker_and_mbr(self):
        self.fake.corrupt_stock_verify = True
        with self.assertRaisesRegex(ValueError, 'device readback differs'):
            self.run_fake()
        connection = self.fake.instances[-1]
        self.assertEqual(connection.writes, [2, 3, 4, 5])
        self.assertEqual(connection.arms, [(2, 4)])
        self.assertEqual(connection.data[:512], bytes(512))
        self.assertEqual(connection.data[6 * 512:7 * 512], bytes(512))
        self.assertEqual(self.events()[-1]['event'], 'stopped_uncertain')
        self.assertFalse(any(e['event'] == 'complete' for e in self.events()))

    def test_lost_ack_is_not_replayed_and_intent_remains_uncertain(self):
        self.fake.fail_write = 3
        with self.assertRaisesRegex(TimeoutError, 'completion uncertain'):
            self.run_fake()
        connection = self.fake.instances[-1]
        self.assertEqual(connection.writes, [2, 3])
        self.assertEqual(connection.data[3 * 512:4 * 512], b'S' * 512)
        self.assertTrue(connection.closed)
        self.assertTrue(any(e['event'] == 'write_intent' and e['sector'] == 3 for e in self.events()))
        self.assertFalse(any(e['event'] == 'write_verified' and e['sector'] == 3 for e in self.events()))
        self.assertEqual(self.events()[-1]['event'], 'stopped_uncertain')

    def test_invalid_extent_hash_fails_real_preflight_without_opening_usb(self):
        extent = self.root / 'bad-extent.bin'
        extent.write_bytes(bytes(512))
        self.plan_path.write_text(json.dumps({
            'version': 1, 'complete': True,
            'layout': executor.Layout().__dict__, 'changed_sectors': 1,
            'extents': [{'phase': 'stock', 'start': 2048, 'count': 1,
                         'file': extent.name, 'sha256': '0' * 64}]}))
        with patch.object(executor, 'Connection') as connection:
            with self.assertRaisesRegex(ValueError, 'SHA256 differs'):
                executor.apply(self.plan_path, self.journal, 'MUST-NOT-OPEN')
            connection.assert_not_called()
        self.assertFalse(self.journal.exists())

    def test_existing_journal_prevents_another_connection(self):
        self.journal.write_text('previous attempt must be preserved\n')
        with patch.object(executor, 'preflight', return_value=self.preflight_result), \
                patch.object(executor, 'Connection') as connection:
            with self.assertRaises(FileExistsError):
                executor.apply(self.plan_path, self.journal, 'MUST-NOT-OPEN')
            connection.assert_not_called()
        self.assertEqual(self.journal.read_text(), 'previous attempt must be preserved\n')

    def test_other_geometry_fails_preflight_before_journal_or_connection(self):
        self.plan_path.write_text(json.dumps({
            'version': 1, 'complete': True,
            'layout': self.preflight_result[1].__dict__,
            'changed_sectors': 0, 'extents': []}))
        with patch.object(executor, 'Connection') as connection:
            with self.assertRaisesRegex(ValueError, 'only the reviewed C606 geometry'):
                executor.apply(self.plan_path, self.journal, 'MUST-NOT-OPEN')
            connection.assert_not_called()
        self.assertFalse(self.journal.exists())

    def test_written_mode_samples_before_marker_and_does_not_claim_full_stock_hash(self):
        self.run_fake(verification='written')
        connection = self.fake.instances[-1]
        self.assertEqual(connection.writes, [2, 3, 4, 5, 6, 0])
        self.assertNotIn((2, 4), connection.reads)
        self.assertIn((2, 1), connection.reads)
        self.assertIn((5, 1), connection.reads)
        events = self.events()
        order = [(e['event'], e.get('phase')) for e in events]
        self.assertLess(order.index(('target_samples_verified', 'stock')), order.index(('arm_intent', 'marker')))
        self.assertFalse(any(e['event'] == 'target_verified' and e.get('phase') == 'stock' for e in events))
        self.assertEqual(events[-1]['stock_verification'], executor.WRITTEN_VERIFICATION)
        self.assertNotIn('stock_sha256', events[-1])

    def test_written_metadata_or_boundary_failure_blocks_marker_and_mbr(self):
        for attribute in ('corrupt_stock_sample', 'corrupt_boundary'):
            with self.subTest(attribute=attribute):
                setattr(self.fake, attribute, True)
                self.journal = self.root / (attribute + '.jsonl')
                with self.assertRaises(ValueError):
                    self.run_fake(verification='written')
                connection = self.fake.instances[-1]
                self.assertEqual(connection.writes, [2, 3, 4, 5])
                self.assertEqual(connection.arms, [(2, 4)])
                self.assertEqual(self.events()[-1]['event'], 'stopped_uncertain')
                setattr(self.fake, attribute, False)

    def test_stock_samples_include_boot_fsinfo_fat_edges_root_and_end(self):
        boot = bytearray(512)
        boot[13], boot[16] = 8, 2
        struct.pack_into('<H', boot, 14, 32)
        struct.pack_into('<I', boot, 36, 10)
        struct.pack_into('<I', boot, 44, 2)
        struct.pack_into('<HH', boot, 48, 1, 6)
        path = self.root / 'sample-boot.bin'
        path.write_bytes(boot)
        samples = executor.stock_sample_sectors({'path': path, 'count': 1000})
        self.assertEqual(samples, [0, 1, 6, 7, 32, 41, 42, 51, 52, 59, 999])

    def test_single_read_exception_method_is_recorded_without_promoting_backup(self):
        plan = self.preflight_result[0]
        plan['backup_verification_method'] = 'single_media_read_with_matching_copy'
        copy = self.root / 'separate-copy.bin'
        copy.write_bytes(self.baseline.read_bytes())
        self.run_fake(single_read_copy=copy, verification='written')
        event = self.events()[0]
        self.assertEqual(event['backup_verification_method'], 'single_media_read_with_matching_copy')
        self.assertEqual(event['single_read_copy'], str(copy.resolve()))


if __name__ == '__main__':
    unittest.main()
