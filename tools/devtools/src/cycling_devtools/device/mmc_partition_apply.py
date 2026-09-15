#!/usr/bin/env python3
"""Reviewed C606 partition executor. Offline validation is the default."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import time

from cycling_devtools.device.mmc_maintenance import (Connection, Progress, SECTOR, digest_file, private_path,
                             validate_backup)
from cycling_devtools.device.mmc_partition import Layout, make_marker, make_mbr, stock_geometry, verified_extents

CHUNK = 1024 * 1024
WRITTEN_VERIFICATION = 'per-written-sector-readback-plus-metadata-samples'


def exact_read(source, offset, size):
    source.seek(offset)
    result = source.read(size)
    if len(result) != size:
        raise ValueError('offline source shortened; no further device operation allowed')
    return result


def compare_ranges(left, left_offset, right, right_offset, size):
    for offset in range(0, size, CHUNK):
        count = min(CHUNK, size - offset)
        if exact_read(left, left_offset + offset, count) != exact_read(right, right_offset + offset, count):
            raise ValueError('plan extents do not reconstruct the exact target from the verified baseline')


def preflight(plan_path, single_read_copy=None):
    # Materialize this iterator: its whole validation must finish before USB opens.
    extents = list(verified_extents(plan_path))
    plan = json.loads(plan_path.read_text())
    layout = Layout(**plan['layout'])
    if layout != Layout():
        raise ValueError('executor accepts only the reviewed C606 geometry')
    backup_manifest = private_path(plan['baseline_manifest'])
    method = validate_backup(backup_manifest, layout.total, single_read_copy=single_read_copy)
    plan['backup_verification_method'] = method
    backup = json.loads(backup_manifest.read_text())
    baseline = private_path(backup['image'])
    if baseline != private_path(plan['baseline_image']) or backup['sha256'] != plan['baseline_sha256']:
        raise ValueError('plan is not bound to this independently verified baseline')
    expected_ranges = {'stock': (layout.stock_start, layout.stock_sectors),
                       'marker': (layout.custom_start, 1), 'mbr_commit': (0, 1)}
    records = plan['targets']
    if len(records) != 3 or [r['phase'] for r in records] != list(expected_ranges):
        raise ValueError('plan must contain stock, marker and final MBR targets in order')
    targets = {}
    for record in records:
        phase = record['phase']
        path = private_path(record['path'])
        start, count = expected_ranges[phase]
        if (record['start'], record['count']) != (start, count):
            raise ValueError('target range differs from reviewed layout')
        if path.stat().st_size != count * SECTOR or digest_file(path) != record['sha256']:
            raise ValueError('target content differs from completed plan')
        targets[phase] = {**record, 'path': path}
    stock_geometry(targets['stock']['path'], layout)
    mbr = make_mbr(layout)
    if targets['mbr_commit']['path'].read_bytes() != mbr:
        raise ValueError('MBR is not the exact reviewed two-partition table')
    if targets['marker']['path'].read_bytes() != make_marker(layout, mbr):
        raise ValueError('ownership marker differs from reviewed MBR and geometry')
    for phase in ('marker', 'mbr_commit'):
        phase_extents = [e for e in extents if e['phase'] == phase]
        if len(phase_extents) != 1 or phase_extents[0]['count'] != 1:
            raise ValueError('this first-time conversion requires exactly one marker and MBR extent')
    # Prove that no target changes are omitted and no extent differs from its
    # target. Gaps must already match the baseline; all of this is offline.
    with baseline.open('rb') as original:
        for phase, target in targets.items():
            cursor = target['start']
            end = cursor + target['count']
            with target['path'].open('rb') as desired:
                for extent in (e for e in extents if e['phase'] == phase):
                    gap = extent['start'] - cursor
                    compare_ranges(original, cursor * SECTOR, desired,
                                   (cursor - target['start']) * SECTOR, gap * SECTOR)
                    with extent['path'].open('rb') as delta:
                        compare_ranges(delta, 0, desired,
                                       (extent['start'] - target['start']) * SECTOR,
                                       extent['count'] * SECTOR)
                    cursor = extent['start'] + extent['count']
                compare_ranges(original, cursor * SECTOR, desired,
                               (cursor - target['start']) * SECTOR, (end - cursor) * SECTOR)
    return plan, layout, baseline, targets, extents


class Journal:
    def __init__(self, path):
        # Existing journals are never resumed or overwritten, including failures.
        self.output = path.open('x', encoding='utf-8')
        os.chmod(path, 0o600)
        fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

    def record(self, event, **fields):
        self.output.write(json.dumps({'at': time.time(), 'event': event, **fields}, sort_keys=True) + '\n')
        self.output.flush()
        os.fsync(self.output.fileno())

    def close(self):
        self.output.close()


def write_extent(connection, extent, target, journal, progress):
    # Freeze this bounded extent before ARM, rechecking both its hash and target.
    data = extent['path'].read_bytes()
    if len(data) != extent['count'] * SECTOR or hashlib.sha256(data).hexdigest() != extent['sha256']:
        raise ValueError('extent changed since preflight; stopped before arming it')
    with target['path'].open('rb') as desired:
        if data != exact_read(desired, (extent['start'] - target['start']) * SECTOR, len(data)):
            raise ValueError('target changed since preflight; stopped before arming extent')
    journal.record('arm_intent', phase=extent['phase'], start=extent['start'], count=extent['count'])
    connection.arm(extent['start'], extent['count'])
    journal.record('armed', phase=extent['phase'], start=extent['start'], count=extent['count'])
    for index in range(extent['count']):
        sector = extent['start'] + index
        payload = data[index * SECTOR:(index + 1) * SECTOR]
        journal.record('write_intent', phase=extent['phase'], sector=sector)
        # Exactly one submission. The device acknowledges only after programming
        # completion, CMD13 and byte-for-byte readback. Any exception propagates.
        connection.write_sector(sector, payload)
        journal.record('write_verified', phase=extent['phase'], sector=sector)
        progress.add(SECTOR)


def verify_target(connection, target, journal):
    journal.record('target_verify_begin', phase=target['phase'], start=target['start'], count=target['count'])
    digest = hashlib.sha256()
    progress = Progress('verify ' + target['phase'], target['count'] * SECTOR)
    completed = 0
    with target['path'].open('rb') as desired:
        for data in connection.read(target['start'], target['count']):
            if not data or len(data) % SECTOR or completed + len(data) > target['count'] * SECTOR:
                raise ValueError('invalid target verification frame')
            if desired.read(len(data)) != data:
                raise ValueError(f"device readback differs from {target['phase']} at relative byte {completed}")
            digest.update(data)
            completed += len(data)
            progress.add(len(data))
        if completed != target['count'] * SECTOR or desired.read(1) or digest.hexdigest() != target['sha256']:
            raise ValueError('target verification length or completed-plan SHA256 mismatch')
    journal.record('target_verified', phase=target['phase'], bytes=completed, sha256=digest.hexdigest())


def stock_sample_sectors(target):
    with target['path'].open('rb') as source:
        boot = exact_read(source, 0, SECTOR)
    reserved = struct.unpack_from('<H', boot, 14)[0]
    fats, spc = boot[16], boot[13]
    fat_size = struct.unpack_from('<I', boot, 36)[0]
    root = struct.unpack_from('<I', boot, 44)[0] & 0x0fffffff
    fsinfo, backup_boot = struct.unpack_from('<HH', boot, 48)
    root_start = reserved + fats * fat_size + (root - 2) * spc
    samples = {0, backup_boot, fsinfo, root_start, root_start + spc - 1, target['count'] - 1}
    if backup_boot + fsinfo < reserved:
        samples.add(backup_boot + fsinfo)
    for index in range(fats):
        samples.update((reserved + index * fat_size, reserved + (index + 1) * fat_size - 1))
    if not samples or any(sector < 0 or sector >= target['count'] for sector in samples):
        raise ValueError('stock metadata samples exceed target partition')
    return sorted(samples)


def verify_stock_samples(connection, target, journal):
    samples = stock_sample_sectors(target)
    journal.record('target_sample_verify_begin', phase='stock', method=WRITTEN_VERIFICATION, sectors=samples)
    with target['path'].open('rb') as desired:
        for relative in samples:
            data = b''.join(connection.read(target['start'] + relative, 1))
            if data != exact_read(desired, relative * SECTOR, SECTOR):
                raise ValueError(f'stock metadata sample differs at relative sector {relative}')
    journal.record('target_samples_verified', phase='stock', method=WRITTEN_VERIFICATION, sectors=samples)


def verify_unmodified_boundaries(connection, baseline, layout, journal):
    samples = (layout.stock_start - 1, layout.custom_start, layout.total - 1)
    with baseline.open('rb') as original:
        for sector in samples:
            if b''.join(connection.read(sector, 1)) != exact_read(original, sector * SECTOR, SECTOR):
                raise ValueError('unchanged boundary differs before marker and MBR commit')
    journal.record('unchanged_boundaries_verified', sectors=samples)


def apply(plan_path, journal_path, port, single_read_copy=None, verification='full'):
    if verification not in ('full', 'written'):
        raise ValueError('unknown stock verification policy')
    plan, layout, baseline, targets, extents = preflight(plan_path, single_read_copy=single_read_copy)
    journal = Journal(journal_path)
    try:
        journal.record('preflight_verified', plan=str(plan_path), plan_sha256=digest_file(plan_path),
                       baseline_sha256=plan['baseline_sha256'], total_sectors=layout.total,
                       changed_sectors=plan['changed_sectors'],
                       backup_verification_method=plan.get('backup_verification_method', 'independent_full_media_read'),
                       stock_verification=verification,
                       single_read_copy=str(Path(single_read_copy).resolve()) if single_read_copy is not None else None)
        with Connection(port) as connection:
            info = connection.info()
            if info['total_sectors'] != layout.total:
                raise ValueError('connected medium capacity differs from verified backup')
            # Applying a plan requires no intervening writes or stock boots
            # since its verified capture. These sentinels detect a changed layout
            # or already-applied plan; they do not prove whole-medium provenance.
            with baseline.open('rb') as original:
                for sector in (0, layout.custom_start, layout.total - 1):
                    actual = b''.join(connection.read(sector, 1))
                    if actual != exact_read(original, sector * SECTOR, SECTOR):
                        raise ValueError('live baseline sentinel differs; do not apply or retry')
            journal.record('live_baseline_sentinels_verified')
            progress = Progress('partition writes verified', plan['changed_sectors'] * SECTOR)
            for extent in (e for e in extents if e['phase'] == 'stock'):
                write_extent(connection, extent, targets['stock'], journal, progress)
            # Full verification remains the default. The explicit written mode
            # keeps per-write device readback and checks metadata/boundaries; it
            # does not establish the complete target's device-side SHA256.
            if verification == 'full':
                verify_target(connection, targets['stock'], journal)
            else:
                verify_stock_samples(connection, targets['stock'], journal)
            verify_unmodified_boundaries(connection, baseline, layout, journal)
            for phase in ('marker', 'mbr_commit'):
                extent = next(e for e in extents if e['phase'] == phase)
                write_extent(connection, extent, targets[phase], journal, progress)
                verify_target(connection, targets[phase], journal)
            journal.record('complete', stock_target_sha256=targets['stock']['sha256'],
                           stock_verification='full_target_readback' if verification == 'full' else WRITTEN_VERIFICATION,
                           marker_sha256=targets['marker']['sha256'], mbr_sha256=targets['mbr_commit']['sha256'])
    except BaseException as error:
        # Never recover, replay, roll back or boot stock automatically. Failure
        # can mean a completed write with a lost response; journal intent matters.
        try:
            journal.record('stopped_uncertain', error=type(error).__name__, detail=str(error))
        except BaseException:
            pass
        raise
    finally:
        journal.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', type=private_path, required=True)
    parser.add_argument('--journal', type=private_path)
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--execute', action='store_true', help='perform the validated one-shot conversion; no retries')
    parser.add_argument('--single-read-copy', type=Path,
                        help='explicitly accept one complete media read with a separate matching copy; does not mark backup verified')
    parser.add_argument('--verification', choices=('full', 'written'), default='full',
                        help='stock verification: full readback, or explicit per-write readback plus metadata samples')
    args = parser.parse_args()
    os.umask(0o077)
    if not args.execute:
        plan, _, _, _, extents = preflight(args.plan, single_read_copy=args.single_read_copy)
        print(f"Validated offline: {len(extents)} extents, {plan['changed_sectors']} changed sectors. No device opened.")
        return
    if args.journal is None:
        parser.error('--execute requires a new --journal path')
    apply(args.plan, args.journal, args.port, single_read_copy=args.single_read_copy, verification=args.verification)
    print(f'Conversion completed with stock verification={args.verification}; stock has not been booted.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(f'Stopped without retry: {error}', file=sys.stderr)
        raise SystemExit(1)
