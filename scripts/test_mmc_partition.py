"""Offline partition layout, marker, and exact changed-sector planning checks."""
import json
import shutil
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import mmc_partition as part
import mmc_maintenance as maintenance
from mmc_maintenance import crc


def fat32_file(path, layout):
    boot = bytearray(512)
    boot[:3] = b'\xeb\x58\x90'
    struct.pack_into('<H', boot, 11, 512)
    boot[13] = 1
    struct.pack_into('<H', boot, 14, 32)
    boot[16] = 1
    struct.pack_into('<I', boot, 28, layout.stock_start)
    struct.pack_into('<I', boot, 32, layout.stock_sectors)
    struct.pack_into('<I', boot, 36, 550)
    struct.pack_into('<I', boot, 44, 2)
    struct.pack_into('<HH', boot, 48, 1, 6)
    boot[510:] = b'\x55\xaa'
    info = bytearray(512)
    info[:4] = b'RRaA'
    info[484:488] = b'rrAa'
    info[508:] = b'\0\0\x55\xaa'
    struct.pack_into('<II', info, 488, 0xffffffff, 0xffffffff)
    with path.open('wb') as output:
        output.write(boot)
        output.write(info)
        output.seek(6 * 512)
        output.write(boot)
        output.truncate(layout.stock_sectors * 512)


class PartitionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_agreed_layout_mbr_and_marker(self):
        layout = part.Layout()
        self.assertEqual(layout.stock_sectors * 512, 1000341504)
        self.assertEqual(layout.custom_start + layout.custom_sectors, 7733248)
        mbr = part.make_mbr(layout)
        self.assertEqual(mbr[510:], b'\x55\xaa')
        self.assertEqual(mbr[:446], b'\0' * 446)
        self.assertEqual(mbr[478:510], b'\0' * 32)
        self.assertEqual((mbr[446], mbr[462]), (0, 0))
        self.assertEqual((mbr[450], mbr[466]), (0x0c, 0xda))
        self.assertEqual(struct.unpack_from('<II', mbr, 454), (2048, 1953792))
        self.assertEqual(struct.unpack_from('<II', mbr, 470), (1955840, 5777408))
        marker = part.make_marker(layout, mbr)
        self.assertEqual(marker[:16], b'CYCLING-BULK\0\0\0\0')
        self.assertEqual(struct.unpack_from('<II5QI', marker, 16),
                         (1, 512, 7733248, 2048, 1953792, 1955840, 5777408, crc(mbr)))
        self.assertEqual(marker[68:508], b'\0' * 440)
        self.assertEqual(struct.unpack_from('<I', marker, 508)[0], crc(marker[:508]))

    def test_invalid_overlap_and_bounds(self):
        for layout in (part.Layout(stock_start=0), part.Layout(stock_sectors=2000000),
                       part.Layout(custom_sectors=1), part.Layout(total=7733247),
                       part.Layout(total=2**32)):
            with self.subTest(layout=layout):
                with self.assertRaises(ValueError):
                    part.make_mbr(layout)
        with self.assertRaises(ValueError):
            part.make_marker(part.Layout(), b'\0' * 512)

    def test_stock_geometry_rejects_size_hidden_sector_and_backup_mismatch(self):
        layout = part.Layout(total=140100, stock_sectors=70000, custom_start=72048, custom_sectors=68052)
        stock = self.root / 'stock.img'
        fat32_file(stock, layout)
        self.assertEqual(part.stock_geometry(stock, layout)['bits'], 32)
        with stock.open('r+b') as output:
            output.seek(28)
            output.write(struct.pack('<I', 0))
        with self.assertRaisesRegex(ValueError, 'hidden-sector'):
            part.stock_geometry(stock, layout)
        fat32_file(stock, layout)
        with stock.open('r+b') as output:
            output.seek(6 * 512)
            output.write(b'X')
        with self.assertRaisesRegex(ValueError, 'backup boot'):
            part.stock_geometry(stock, layout)
        with stock.open('r+b') as output:
            output.truncate(512)
        with self.assertRaisesRegex(ValueError, 'exactly fill'):
            part.stock_geometry(stock, layout)

    def make_diff(self):
        baseline = self.root / 'baseline.img'
        original = b''.join(bytes([index]) * 512 for index in range(20))
        baseline.write_bytes(original)
        stock = self.root / 'stock.img'
        stock.write_bytes(b'a' * 1024 + original[4 * 512:5 * 512] + b'b' * 1536)
        marker = self.root / 'marker.bin'
        marker.write_bytes(b'm' * 512)
        mbr = self.root / 'mbr.bin'
        mbr.write_bytes(b'p' * 512)
        output = self.root / 'plan'
        output.mkdir()
        targets = [{'phase': 'stock', 'start': 2, 'path': stock},
                   {'phase': 'marker', 'start': 10, 'path': marker},
                   {'phase': 'mbr_commit', 'start': 0, 'path': mbr}]
        extents, records = part.plan_differences(baseline, targets, output, 20, max_sectors=2)
        return original, output, targets, extents, records

    def test_diff_replay_changes_only_exact_target_sectors_and_mbr_last(self):
        original, output, targets, extents, records = self.make_diff()
        changed = bytearray(original)
        touched = set()
        for extent in extents:
            start, count = extent['start'], extent['count']
            changed[start * 512:(start + count) * 512] = (output / extent['file']).read_bytes()
            touched.update(range(start, start + count))
            self.assertLessEqual(count, 2)
        self.assertEqual(touched, {0, 2, 3, 5, 6, 7, 10})
        self.assertEqual([row['phase'] for row in records], ['stock', 'marker', 'mbr_commit'])
        self.assertEqual(extents[-1]['phase'], 'mbr_commit')
        for target in targets:
            data = target['path'].read_bytes()
            start = target['start'] * 512
            self.assertEqual(changed[start:start + len(data)], data)
        for index in range(20):
            if index not in touched:
                self.assertEqual(changed[index * 512:(index + 1) * 512], original[index * 512:(index + 1) * 512])

    def test_no_changes_produce_no_extents(self):
        baseline = self.root / 'baseline'
        target = self.root / 'target'
        baseline.write_bytes(b'x' * 1024)
        target.write_bytes(b'x' * 512)
        extents, records = part.plan_differences(baseline, [{'phase': 'stock', 'start': 1, 'path': target}], self.root, 2)
        self.assertEqual(extents, [])
        self.assertEqual(records[0]['count'], 1)

    def test_invalid_target_sizes_overlap_and_bounds(self):
        baseline = self.root / 'baseline'
        baseline.write_bytes(b'x' * 1024)
        target = self.root / 'target'
        target.write_bytes(b'z' * 512)
        for targets in ([{'phase': 'stock', 'start': 2, 'path': target}],
                        [{'phase': 'stock', 'start': 1, 'path': target}] * 2):
            with self.assertRaises(ValueError):
                part.plan_differences(baseline, targets, self.root, 2)
        target.write_bytes(b'x')
        with self.assertRaises(ValueError):
            part.plan_differences(baseline, [{'phase': 'stock', 'start': 0, 'path': target}], self.root, 2)

    def test_verified_iterator_checks_all_files_before_first_yield(self):
        _, output, _, extents, _ = self.make_diff()
        plan = {'version': 1, 'complete': True,
                'layout': {'total': 20, 'stock_start': 2, 'stock_sectors': 8,
                           'custom_start': 10, 'custom_sectors': 10},
                'extents': extents, 'changed_sectors': sum(e['count'] for e in extents)}
        plan_path = output / 'plan.json'
        plan_path.write_text(json.dumps(plan))
        self.assertEqual(len(list(part.verified_extents(plan_path))), len(extents))
        (output / extents[-1]['file']).write_bytes(b'x' * 512)
        iterator = part.verified_extents(plan_path)
        with self.assertRaises(ValueError):
            next(iterator)

    def test_verified_iterator_rejects_outside_region_and_commit_order(self):
        _, output, _, extents, _ = self.make_diff()
        plan = {'version': 1, 'complete': True,
                'layout': {'total': 20, 'stock_start': 2, 'stock_sectors': 8,
                           'custom_start': 10, 'custom_sectors': 10},
                'extents': extents, 'changed_sectors': sum(e['count'] for e in extents)}
        plan_path = output / 'plan.json'
        plan['extents'] = list(reversed(extents))
        plan_path.write_text(json.dumps(plan))
        with self.assertRaises(ValueError):
            list(part.verified_extents(plan_path))
        plan['extents'] = [{**extents[0], 'start': 1}]
        plan_path.write_text(json.dumps(plan))
        with self.assertRaises(ValueError):
            list(part.verified_extents(plan_path))

    def test_prepare_requires_verified_backup_and_finishes_self_contained_extents(self):
        local = self.root / '.local'
        local.mkdir()
        layout = part.Layout(total=72100, stock_sectors=70000, custom_start=72048, custom_sectors=52)
        baseline = local / 'baseline.img'
        with baseline.open('wb') as output:
            output.truncate(layout.total * 512)
        stock = local / 'stock.img'
        fat32_file(stock, layout)
        manifest = local / 'baseline.json'
        metadata = {'version': 1, 'complete': True, 'verified': False, 'image': str(baseline),
                    'total_sectors': layout.total, 'sector_size': 512,
                    'sha256': maintenance.digest_file(baseline)}
        manifest.write_text(json.dumps(metadata))
        destination = local / 'plan'
        with patch.object(maintenance, 'ROOT', self.root):
            with self.assertRaises(ValueError):
                part.prepare(manifest, stock, destination, layout)
            self.assertFalse(destination.exists())
            separate_copy = self.root / 'external-copy.img'
            shutil.copyfile(baseline, separate_copy)
            single = part.prepare(manifest, stock, local / 'single-read-plan', layout,
                                  single_read_copy=separate_copy)
            self.assertEqual(single['backup_verification_method'], 'single_media_read_with_matching_copy')
            self.assertFalse(json.loads(manifest.read_text())['verified'])
            metadata['verified'] = True
            manifest.write_text(json.dumps(metadata))
            result = part.prepare(manifest, stock, destination, layout)
        self.assertTrue(result['complete'])
        verified = list(part.verified_extents(destination / 'plan.json'))
        self.assertEqual(verified[-1]['phase'], 'mbr_commit')
        self.assertEqual(result['changed_sectors'], 5)
        self.assertEqual(result['baseline_sha256'], metadata['sha256'])


if __name__ == '__main__':
    unittest.main()
