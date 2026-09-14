"""Prepare an offline, reviewable C606 partition conversion; no device access."""
import argparse
from dataclasses import asdict, dataclass
import hashlib
import json
import os
from pathlib import Path
import struct
import sys

import mmc
from mmc_maintenance import crc, digest_file, private_path, save_manifest, validate_backup

SECTOR = 512
MAX_EXTENT_SECTORS = 2048


@dataclass(frozen=True)
class Layout:
    total: int = 7_733_248
    stock_start: int = 2048
    stock_sectors: int = 1_953_792
    custom_start: int = 1_955_840
    custom_sectors: int = 5_777_408

    def validate(self):
        values = asdict(self)
        if any(type(value) is not int or value <= 0 or value > 0xffffffff for value in values.values()):
            raise ValueError('layout values must be positive 32-bit sector counts')
        if self.stock_start + self.stock_sectors > self.custom_start:
            raise ValueError('stock and custom partitions overlap')
        if self.custom_start + self.custom_sectors != self.total or self.custom_sectors < 2:
            raise ValueError('custom partition must include marker and end at medium boundary')


def make_mbr(layout):
    layout.validate()
    result = bytearray(SECTOR)
    for index, (kind, start, count) in enumerate(((0x0c, layout.stock_start, layout.stock_sectors),
                                                (0xda, layout.custom_start, layout.custom_sectors))):
        offset = 446 + index * 16
        # LBA entries with saturated CHS coordinates; neither partition is active.
        result[offset + 1:offset + 4] = b'\xfe\xff\xff'
        result[offset + 4] = kind
        result[offset + 5:offset + 8] = b'\xfe\xff\xff'
        struct.pack_into('<II', result, offset + 8, start, count)
    result[510:] = b'\x55\xaa'
    return bytes(result)


def make_marker(layout, mbr):
    if mbr != make_mbr(layout):
        raise ValueError('marker requires the exact planned MBR')
    result = bytearray(SECTOR)
    result[:16] = b'CYCLING-BULK\0\0\0\0'
    struct.pack_into('<II5QI', result, 16, 1, SECTOR, layout.total, layout.stock_start,
                     layout.stock_sectors, layout.custom_start, layout.custom_sectors, crc(mbr))
    struct.pack_into('<I', result, 508, crc(result[:508]))
    return bytes(result)


def stock_geometry(path, layout):
    layout.validate()
    if path.stat().st_size != layout.stock_sectors * SECTOR:
        raise ValueError('stock image must exactly fill planned first partition')
    with path.open('rb') as source:
        def sector(index):
            source.seek(index * SECTOR)
            return source.read(SECTOR)
        media = mmc.Media(layout.stock_sectors, sector)
        geometry = mmc.fat_geometry(media, {'start': 0, 'sectors': layout.stock_sectors})
        if geometry is None or geometry['bits'] != 32 or geometry['sectors'] != layout.stock_sectors:
            raise ValueError('stock image must be FAT32 with exact planned BPB sector count')
        boot = media.read(0)
        if struct.unpack_from('<I', boot, 28)[0] != layout.stock_start:
            raise ValueError('FAT32 hidden-sector field must match planned partition start')
        reserved = struct.unpack_from('<H', boot, 14)[0]
        fsinfo, boot_backup = struct.unpack_from('<HH', boot, 48)
        if not 0 < fsinfo < reserved or not 0 < boot_backup < reserved:
            raise ValueError('FAT32 FSInfo and backup boot sectors must be within reserved region')
        if media.read(boot_backup) != boot:
            raise ValueError('FAT32 backup boot sector differs from primary')
        info = media.read(fsinfo)
        if (info[:4] != b'RRaA' or info[484:488] != b'rrAa'
                or info[508:512] != b'\0\0\x55\xaa'):
            raise ValueError('invalid FAT32 FSInfo signatures')
        free, next_free = struct.unpack_from('<II', info, 488)
        if free != 0xffffffff and free > geometry['clusters']:
            raise ValueError('invalid FAT32 free-cluster count')
        if next_free != 0xffffffff and not 2 <= next_free < geometry['clusters'] + 2:
            raise ValueError('invalid FAT32 next-free cluster')
    return geometry


def write_new(path, data):
    with path.open('xb') as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())


def plan_differences(baseline, targets, output, total, max_sectors=MAX_EXTENT_SECTORS):
    """Create exact changed runs in target order; untouched sectors have no extent.

    targets are dictionaries with phase, start and path. All paths and the output
    directory are caller-owned offline files. Each target must contain whole
    sectors, and targets must not overlap. No target data is read from a device.
    """
    if not 1 <= max_sectors <= MAX_EXTENT_SECTORS:
        raise ValueError('invalid extent bound')
    if baseline.stat().st_size != total * SECTOR:
        raise ValueError('baseline size does not match medium')
    ranges = []
    for target in targets:
        size = target['path'].stat().st_size
        start = target['start']
        if size == 0 or size % SECTOR or start < 0 or start + size // SECTOR > total:
            raise ValueError('target range is invalid or outside medium')
        ranges.append((start, start + size // SECTOR))
    ordered = sorted(ranges)
    if any(left[1] > right[0] for left, right in zip(ordered, ordered[1:])):
        raise ValueError('target ranges overlap')
    extents, target_records = [], []
    with baseline.open('rb') as original:
        for target_index, target in enumerate(targets):
            path, start = target['path'], target['start']
            expected_count = ranges[target_index][1] - start
            digest = hashlib.sha256()
            run = bytearray()
            run_start = None
            def flush():
                nonlocal run, run_start
                if not run:
                    return
                name = f'extent-{len(extents):06d}-{run_start:08d}.bin'
                write_new(output / name, run)
                extents.append({'phase': target['phase'], 'start': run_start,
                                'count': len(run) // SECTOR, 'file': name,
                                'sha256': hashlib.sha256(run).hexdigest()})
                run, run_start = bytearray(), None
            original.seek(start * SECTOR)
            with path.open('rb') as desired:
                current = start
                while True:
                    data = desired.read(MAX_EXTENT_SECTORS * SECTOR)
                    if not data:
                        break
                    if len(data) % SECTOR:
                        raise ValueError('target changed to a partial sector during planning')
                    old = original.read(len(data))
                    if len(old) != len(data):
                        raise ValueError('baseline shortened during planning')
                    digest.update(data)
                    for offset in range(0, len(data), SECTOR):
                        sector = data[offset:offset + SECTOR]
                        if sector == old[offset:offset + SECTOR]:
                            flush()
                        else:
                            if run_start is None:
                                run_start = current
                            run.extend(sector)
                            if len(run) == max_sectors * SECTOR:
                                flush()
                        current += 1
                flush()
                if (current - start != expected_count
                        or path.stat().st_size != expected_count * SECTOR):
                    raise ValueError('target size changed during planning')
            target_records.append({'phase': target['phase'], 'path': str(path), 'start': start,
                                   'count': current - start, 'sha256': digest.hexdigest()})
    return extents, target_records


def prepare(backup_manifest, stock, output, layout=Layout(), single_read_copy=None):
    layout.validate()
    method = validate_backup(backup_manifest, layout.total, single_read_copy=single_read_copy)
    baseline_manifest = json.loads(backup_manifest.read_text())
    baseline = private_path(baseline_manifest['image'])
    geometry = stock_geometry(stock, layout)
    output.mkdir(mode=0o700)  # A failed plan remains private and is never reused.
    manifest = {'version': 1, 'complete': False, 'layout': asdict(layout),
                'baseline_manifest': str(backup_manifest), 'baseline_image': str(baseline),
                'baseline_sha256': baseline_manifest['sha256'], 'fat32_geometry': geometry,
                'backup_verification_method': method}
    if single_read_copy is not None:
        manifest['single_read_copy'] = str(Path(single_read_copy).resolve())
    save_manifest(output / 'plan.json', manifest)
    mbr = make_mbr(layout)
    write_new(output / 'mbr.bin', mbr)
    write_new(output / 'marker.bin', make_marker(layout, mbr))
    targets = [{'phase': 'stock', 'start': layout.stock_start, 'path': stock},
               {'phase': 'marker', 'start': layout.custom_start, 'path': output / 'marker.bin'},
               {'phase': 'mbr_commit', 'start': 0, 'path': output / 'mbr.bin'}]
    extents, records = plan_differences(baseline, targets, output, layout.total)
    manifest.update(complete=True, targets=records, extents=extents,
                    changed_sectors=sum(extent['count'] for extent in extents))
    manifest['write_bytes'] = manifest['changed_sectors'] * SECTOR
    save_manifest(output / 'plan.json', manifest)
    return manifest


def verified_extents(plan_path):
    """Validate all extent files before yielding their ordered write descriptions.

    The executor must separately identify the medium and validate its baseline
    backup before arming writes. This iterator never opens a device or authorizes
    writes by itself. MBR commit must be the final phase.
    """
    plan = json.loads(plan_path.read_text())
    if plan.get('version') != 1 or plan.get('complete') is not True:
        raise ValueError('partition plan is incomplete or unsupported')
    layout = Layout(**plan['layout'])
    layout.validate()
    phases = {'stock': 0, 'marker': 1, 'mbr_commit': 2}
    prior_phase, prior_end = -1, 0
    extents = []
    for extent in plan['extents']:
        phase, start, count = extent['phase'], extent['start'], extent['count']
        index = phases.get(phase, -1)
        ranges = {'stock': (layout.stock_start, layout.stock_sectors),
                  'marker': (layout.custom_start, 1), 'mbr_commit': (0, 1)}
        if index < 0 or index < prior_phase or not 1 <= count <= MAX_EXTENT_SECTORS:
            raise ValueError('invalid write phase or extent size')
        begin, length = ranges[phase]
        if start < begin or start + count > begin + length or (index == prior_phase and start < prior_end):
            raise ValueError('extent outside authorized target or not ordered')
        path = (plan_path.parent / extent['file']).resolve()
        if not path.is_relative_to(plan_path.parent.resolve()) or path.name != extent['file']:
            raise ValueError('extent path must name a file within the plan directory')
        if path.stat().st_size != count * SECTOR or digest_file(path) != extent['sha256']:
            raise ValueError('extent size or SHA256 differs from completed plan')
        extents.append({**extent, 'path': path})
        prior_phase, prior_end = index, start + count
    if sum(item['count'] for item in extents) != plan['changed_sectors']:
        raise ValueError('plan changed-sector total mismatch')
    yield from extents


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backup-manifest', type=private_path, required=True)
    parser.add_argument('--stock-image', type=private_path, required=True)
    parser.add_argument('--output', type=private_path, required=True)
    parser.add_argument('--single-read-copy', type=Path,
                        help='explicitly accept one complete media read with a separate matching copy; does not mark backup verified')
    args = parser.parse_args()
    plan = prepare(args.backup_manifest, args.stock_image, args.output, single_read_copy=args.single_read_copy)
    print(f"Prepared {len(plan['extents'])} extents, {plan['changed_sectors']} sectors, {plan['write_bytes']} bytes; MBR commit last.")


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(f'Stopped: {error}', file=sys.stderr)
        sys.exit(1)
