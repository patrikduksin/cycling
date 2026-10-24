"""Read the immutable ride-slot prefix over USB and create JSON/GPX exports.

`--medium bulk` reads the ride log on the owned MMC region instead of the
internal journal. Its slots are the same version-1 records; each sits in a
sector container the device validates. Invalid containers are exported as zero
slots, listed in the manifest, and their raw sectors are kept for diagnosis.

`--format fit` also writes a FIT activity per ride (`ride-<id>.fit`, listed in
`fit.json`) with the firmware's own encoder, for Strava or Garmin Connect.
`--from <export>` converts an earlier export without the device.

Sample byte 37 marks the first sample of each lap (see `ride/log.rs`). Ride JSON
lists the laps, and GPX marks where each later lap began with a waypoint. A ride
recorded before lap marks has `laps: null`.
"""

import argparse
import datetime as dt
import hashlib
import json
import subprocess
import time
import xml.etree.ElementTree as ET
import zlib
from pathlib import Path

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.little_endian import i16, i32, u16, u32, u64
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT

SLOT_SIZE = 256
COMMIT = 0x434F4D54
MAX_SLOTS = 4096
# Bulk record 0 follows the log's superblocks and checkpoints in its volume.
BULK_LOG_DATA = 16
# Where record 0 was before the volume table (exports made then record no start).
BULK_DATA_START = BULK_LOG_DATA
MEDIA = ('internal', 'bulk')
LAP_STARTS = {0: 'ride', 1: 'manual', 2: 'distance', 3: 'step', 4: 'distance_step'}


def prefix(medium):
    if medium not in MEDIA:
        raise ValueError(f'unknown medium {medium}')
    return 'EXPORT BULK' if medium == 'bulk' else 'EXPORT'


def export_command(connection, command, medium='internal'):
    reply = connection.terminal_command(f'{prefix(medium)} {command}')
    if reply['status'] != 'OK':
        raise RuntimeError(f'export rejected: {reply["status"]}')
    return ['CYCLING_EXPORT', str(reply['id']), *reply['data'].split()]


def info(connection, medium='internal'):
    parts = export_command(connection, 'INFO', medium)
    bulk = medium == 'bulk'
    if len(parts) != (10 if bulk else 7) or parts[2] != 'INFO':
        raise ValueError('malformed export info')
    version, slot_size, upper = map(int, parts[3:6])
    metadata = {'version': version, 'slot_size': slot_size, 'upper_bound': upper, 'status': parts[6]}
    limit = MAX_SLOTS
    if bulk:
        try:
            int(parts[7], 16)
            metadata.update(medium='bulk', store_id=parts[7], generation=int(parts[8]), capacity=int(parts[9]))
        except ValueError as error:
            raise ValueError('malformed bulk export info') from error
        limit = metadata['capacity']
    if version != 1 or slot_size != SLOT_SIZE or not 0 <= upper <= limit:
        raise ValueError('unsupported or out-of-bounds export info')
    return metadata


def read_slot(connection, index, medium='internal'):
    """One slot; bulk returns (slot, container_valid)."""
    parts = export_command(connection, f'SLOT {index}', medium)
    bulk = medium == 'bulk'
    if len(parts) != (7 if bulk else 6) or parts[2] != 'SLOT' or int(parts[3]) != index:
        raise ValueError('wrong slot response')
    try:
        data = bytes.fromhex(parts[5])
        expected = int(parts[4], 16)
    except ValueError as error:
        raise ValueError('malformed slot response') from error
    if len(data) != SLOT_SIZE or zlib.crc32(data) != expected:
        raise ValueError('short or corrupt slot response')
    if not bulk:
        return data
    if parts[6] not in ('valid', 'invalid'):
        raise ValueError('malformed slot container state')
    return data, parts[6] == 'valid'


def bulk_data_start(connection):
    """Owned MMC sector of bulk record 0: the RIDES volume's start plus the log metadata."""
    reply = connection.terminal_command('VOLUME STATUS')
    if reply['status'] == 'INVALID':
        # Firmware from before the volume table has no VOLUME command; its log starts at sector 0.
        return BULK_DATA_START
    fields = dict(field.split('=', 1) for field in reply['data'].split() if '=' in field)
    if reply['status'] != 'OK' or fields.get('layout') != 'volumes' or 'rides' not in fields:
        raise RuntimeError(f'no RIDES volume: {reply["status"]} {reply["data"]}')
    try:
        start = int(fields['rides'].split('+', 1)[0])
    except ValueError as error:
        raise ValueError('malformed volume status') from error
    return start + BULK_LOG_DATA


def read_owned_sector(connection, sector):
    """A raw owned MMC sector, in two CRC-checked chunks, for diagnosis."""
    data = b''
    for offset in (0, 256):
        reply = connection.terminal_command(f'MMC OWNED READ {sector} {offset} 256')
        if reply['status'] != 'OK':
            raise RuntimeError(f'owned read rejected: {reply["status"]}')
        try:
            fields = dict(field.split('=', 1) for field in reply['data'].split())
            chunk = bytes.fromhex(fields['data'])
            ok = int(fields['sector']) == sector and zlib.crc32(chunk) == int(fields['crc32'], 16)
        except (KeyError, ValueError) as error:
            raise ValueError('malformed owned sector response') from error
        if not ok or len(chunk) != 256:
            raise ValueError('short or corrupt owned sector response')
        data += chunk
    return data


def download_prefix(connection, partial, upper, medium='internal'):
    """Write the slot prefix; return indices whose bulk container was invalid."""
    invalid = []
    with Path(partial).open('wb') as raw:
        for index in range(upper):
            slot = read_slot(connection, index, medium)
            if medium == 'bulk':
                slot, valid = slot
                if not valid:
                    invalid.append(index)
            raw.write(slot)
            raw.flush()
    return invalid


def decode_slot(data):
    if len(data) != SLOT_SIZE or data == b'\xff' * SLOT_SIZE:
        return None
    count = data[6]
    if (
        data[:4] != b'RIDE'
        or data[4] != 1
        or data[5] not in range(1, 8)
        or data[7] not in (1, 2)
        or count > 4
        or u16(data, 24) != count * 48
        or data[26:28] != b'\0\0'
        or u32(data, 252) != COMMIT
    ):
        raise ValueError('invalid ride record header')
    checked = bytearray(data[:252])
    stored_crc = u32(checked, 28)
    checked[28:32] = b'\0' * 4
    if zlib.crc32(checked) != stored_crc or (data[5] == 2) != (count > 0):
        raise ValueError('invalid ride record checksum or count')
    samples = []
    for index in range(count):
        value = data[32 + index * 48 : 80 + index * 48]
        flags = u32(value, 16)
        if flags & ~0x800001FF or (flags & 4 and not flags & 0x80000000):
            raise ValueError('invalid sample flags')
        battery = value[36] if flags & 32 else None
        if battery is not None and battery > 100:
            raise ValueError('invalid battery value')
        samples.append(
            {
                'active_ms': u64(value, 0),
                'utc_ms': u64(value, 8) if flags & 1 else None,
                'latitude_e7': i32(value, 20) if flags & 2 else None,
                'longitude_e7': i32(value, 24) if flags & 2 else None,
                'demo_speed_mm_s': u32(value, 28) if flags & 4 else None,
                'heart_bpm': u16(value, 32) if flags & 8 else None,
                'cadence_tenths': u16(value, 34) if flags & 16 else None,
                'speed_mm_s': u32(value, 40) if flags & 64 else None,
                'power_watts': u16(value, 38) if flags & 128 else None,
                'gradient_tenths': i16(value, 44) if flags & 256 else None,
                'battery_percent': battery,
                'altitude_m': altitude_m(u16(value, 46)),
                'lap': LAP_STARTS.get(value[37]),
            }
        )
    active_ms = u64(data, 16)
    if samples and samples[-1]['active_ms'] != active_ms:
        raise ValueError('sample/header duration mismatch')
    return {
        'kind': data[5],
        'source': 'demo' if data[7] == 1 else 'live',
        'ride_id': u32(data, 8),
        'sequence': u32(data, 12),
        'active_ms': active_ms,
        'samples': samples,
    }


def altitude_m(code):
    """FIT's altitude encoding in sample bytes 46..48: (metres + 500) * 5, 0xFFFF for none."""
    return None if code == 0xFFFF else round(code / 5 - 500, 1)


def new_run(ride):
    """Start a running stretch: active time advances with the clock only inside one."""
    ride['_untimed'] = []


def date_run(ride, anchor):
    """Date untimed samples that precede the first UTC sample of their running stretch.

    Without pause, gap or reboot between them, active time and UTC advance together,
    so `utc = anchor_utc - (anchor_active - active)` is exact to the device clock.
    """
    for sample in ride['_untimed']:
        utc = anchor['utc_ms'] - (anchor['active_ms'] - sample['active_ms'])
        if sample['active_ms'] <= anchor['active_ms'] and utc >= 0:
            sample['utc_ms'] = utc
            sample['utc_derived'] = True
    ride['_untimed'] = None


def laps(samples):
    """Each lap's first sample and why it began; None for a ride recorded without lap marks."""
    if all(sample['lap'] is None for sample in samples):
        return None
    starts = [
        (index, sample) for index, sample in enumerate(samples) if index == 0 or sample['lap'] not in (None, 'ride')
    ]
    return [
        {
            'lap': number,
            'started_by': 'ride' if index == 0 else sample['lap'],
            'sample': index,
            'active_ms': sample['active_ms'],
            'utc_ms': sample['utc_ms'],
            'latitude_e7': sample['latitude_e7'],
            'longitude_e7': sample['longitude_e7'],
        }
        for number, (index, sample) in enumerate(starts, 1)
    ]


def ended(ride):
    ride['segments'] = [segment for segment in ride['segments'] if segment]
    ride['laps'] = laps(ride['samples'])
    return ride


def rides_from_slots(slots):
    invalid = []
    return list(iter_rides(slots, invalid)), invalid


def iter_rides(slots, invalid):
    """Yield each ride once it ends, from any iterable of slots; `invalid` collects bad slot indices.

    Only the ride being decoded is held, so memory follows the longest ride, not the prefix.
    """
    current, seen = None, set()
    for index, raw in enumerate(slots):
        try:
            entry = decode_slot(raw)
        except ValueError:
            invalid.append(index)
            if current:
                current['gap'] = True
                current['segments'].append([])
                new_run(current)
            continue
        if entry is None:
            if current:
                current['gap'] = True
                current['segments'].append([])
                new_run(current)
            continue
        entry['slot'] = index
        if entry['kind'] == 1:
            if entry['ride_id'] in seen:
                raise ValueError(f'duplicate ride id {entry["ride_id"]}')
            if current:
                current['terminal'] = 'interrupted'
                current['gap'] = True
                yield ended(current)
            seen.add(entry['ride_id'])
            current = {
                'ride_id': entry['ride_id'],
                'source': entry['source'],
                'start_slot': index,
                'end_slot': None,
                'terminal': None,
                'gap': False,
                'records': [index],
                'samples': [],
                'segments': [[]],
                'next_sequence': entry['sequence'] + 1,
                'active_ms': 0,
                '_last_utc': None,
                '_start_identity': hashlib.sha256(raw).hexdigest(),
            }
            new_run(current)
            continue
        if not current:
            continue
        if entry['ride_id'] != current['ride_id'] or entry['source'] != current['source']:
            current['gap'] = True
            if current['segments'][-1]:
                current['segments'].append([])
            new_run(current)
            continue
        current['records'].append(index)
        if entry['sequence'] != current['next_sequence']:
            current['gap'] = True
            if current['segments'][-1]:
                current['segments'].append([])
            new_run(current)
            if entry['sequence'] < current['next_sequence']:
                continue
        current['next_sequence'] = entry['sequence'] + 1
        current['active_ms'] = entry['active_ms']
        if entry['kind'] == 2:
            for sample in entry['samples']:
                current['samples'].append(sample)
                if current['_untimed'] is not None:
                    if sample['utc_ms'] is None:
                        current['_untimed'].append(sample)
                    else:
                        date_run(current, sample)
                segment = current['segments'][-1]
                missing = sample['latitude_e7'] is None
                invalid_location = not missing and not (
                    -900_000_000 <= sample['latitude_e7'] <= 900_000_000
                    and -1_800_000_000 <= sample['longitude_e7'] <= 1_800_000_000
                )
                backwards = (
                    sample['utc_ms'] is not None
                    and current['_last_utc'] is not None
                    and sample['utc_ms'] < current['_last_utc']
                )
                if sample['utc_ms'] is not None:
                    current['_last_utc'] = sample['utc_ms']
                if missing or invalid_location or backwards:
                    if segment:
                        current['segments'].append([])
                    if missing or invalid_location:
                        continue
                current['segments'][-1].append(sample)
        elif entry['kind'] in (3, 4):
            if current['segments'][-1]:
                current['segments'].append([])
            new_run(current)
        elif entry['kind'] in (5, 6, 7):
            current['terminal'] = {5: 'saved', 6: 'recovered', 7: 'full'}[entry['kind']]
            current['end_slot'] = index
            yield ended(current)
            current = None
    if current:
        current['terminal'] = 'interrupted'
        current['gap'] = True
        yield ended(current)


def gpx_time(utc_ms):
    seconds, milliseconds = divmod(utc_ms, 1000)
    stamp = dt.datetime(1970, 1, 1, tzinfo=dt.UTC) + dt.timedelta(seconds=seconds, milliseconds=milliseconds)
    return stamp.isoformat(timespec='milliseconds').replace('+00:00', 'Z')


def write_gpx(ride, path):
    segments = ride['segments']
    if not segments:
        return 'no valid location samples'
    for segment in segments:
        for sample in segment:
            if sample['utc_ms'] is not None:
                try:
                    seconds, milliseconds = divmod(sample['utc_ms'], 1000)
                    dt.datetime(1970, 1, 1, tzinfo=dt.UTC) + dt.timedelta(seconds=seconds, milliseconds=milliseconds)
                except OverflowError, ValueError:
                    return 'UTC timestamp outside GPX host range'
    ns = 'http://www.topografix.com/GPX/1/1'
    ET.register_namespace('', ns)
    root = ET.Element(f'{{{ns}}}gpx', version='1.1', creator='cycling')
    for lap in (ride.get('laps') or [])[1:]:
        if lap['latitude_e7'] is None:
            continue
        lat, lon = lap['latitude_e7'] / 1e7, lap['longitude_e7'] / 1e7
        if not -90 <= lat <= 90 or not -180 <= lon <= 180:
            continue
        point = ET.SubElement(root, f'{{{ns}}}wpt', lat=f'{lat:.7f}', lon=f'{lon:.7f}')
        if lap['utc_ms'] is not None:
            ET.SubElement(point, f'{{{ns}}}time').text = gpx_time(lap['utc_ms'])
        ET.SubElement(point, f'{{{ns}}}name').text = f'LAP {lap["lap"]}'
        ET.SubElement(point, f'{{{ns}}}type').text = lap['started_by']
    track = ET.SubElement(root, f'{{{ns}}}trk')
    ET.SubElement(track, f'{{{ns}}}name').text = f'Ride {ride["ride_id"]} ({ride["source"]})'
    points = 0
    for segment in segments:
        xml_segment = ET.SubElement(track, f'{{{ns}}}trkseg')
        for sample in segment:
            lat, lon = sample['latitude_e7'] / 1e7, sample['longitude_e7'] / 1e7
            if not -90 <= lat <= 90 or not -180 <= lon <= 180:
                continue
            if lon == 180:
                lon = -180
            point = ET.SubElement(xml_segment, f'{{{ns}}}trkpt', lat=f'{lat:.7f}', lon=f'{lon:.7f}')
            if sample['altitude_m'] is not None:
                ET.SubElement(point, f'{{{ns}}}ele').text = f'{sample["altitude_m"]:.1f}'
            if sample['utc_ms'] is not None:
                ET.SubElement(point, f'{{{ns}}}time').text = gpx_time(sample['utc_ms'])
            points += 1
    if points == 0:
        return 'no valid location samples'
    ET.ElementTree(root).write(path, encoding='utf-8', xml_declaration=True)
    return None


def export(port, output, medium='internal'):
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    partial = output / 'ride-slots.bin.partial'
    with UsbConnection(port, output / 'usb.log') as connection:
        metadata = info(connection, medium)
        invalid_containers = download_prefix(connection, partial, metadata['upper_bound'], medium)
        invalid_sectors = []
        data_start = bulk_data_start(connection) if medium == 'bulk' else None
        for index in invalid_containers:
            sector = data_start + index
            raw_sector = read_owned_sector(connection, sector)
            name = f'invalid-sector-{sector}.bin'
            (output / name).write_bytes(raw_sector)
            invalid_sectors.append(
                {'index': index, 'sector': sector, 'file': name, 'sha256': hashlib.sha256(raw_sector).hexdigest()}
            )
        if info(connection, medium) != metadata:
            raise RuntimeError('ride prefix changed during export')
    canonical = output / 'ride-slots.bin'
    partial.replace(canonical)
    extra = (
        {'invalid_containers': invalid_containers, 'invalid_sectors': invalid_sectors, 'data_start': data_start}
        if medium == 'bulk'
        else {}
    )
    return write_exports(output, canonical, {**metadata, **extra})


def read_slots(path, digest):
    """Slots of a raw prefix file, one at a time, hashing the bytes as they are read."""
    with Path(path).open('rb') as raw:
        while chunk := raw.read(SLOT_SIZE):
            digest.update(chunk)
            yield chunk


def write_exports(output, canonical, header):
    """Decode `canonical` ride by ride into JSON/GPX files and a manifest, in bounded memory.

    Each ride's files and manifest entry are written as soon as it ends; the manifest is
    assembled from those entries at the end. Returns the manifest without its ride list.
    """
    digest = hashlib.sha256()
    invalid = []
    entries = output / 'manifest-rides.partial'
    count = 0
    with entries.open('w') as pending:
        for ride in iter_rides(read_slots(canonical, digest), invalid):
            public = {key: value for key, value in ride.items() if key != 'segments' and not key.startswith('_')}
            json_path = output / f'ride-{ride["ride_id"]}.json'
            json_path.write_text(json.dumps(public, indent=2) + '\n')
            gpx_path = output / f'ride-{ride["ride_id"]}.gpx'
            error = write_gpx(ride, gpx_path)
            entry = {
                'ride_id': ride['ride_id'],
                'source': ride['source'],
                'terminal': ride['terminal'],
                'samples': len(ride['samples']),
                'gpx': None if error else gpx_path.name,
                'gpx_error': error,
                'start_identity': ride['_start_identity'],
            }
            pending.write(json.dumps(entry) + '\n')
            count += 1
    manifest = {**header, 'raw_file': canonical.name, 'raw_sha256': digest.hexdigest(), 'invalid_slots': invalid}
    partial = output / 'manifest.json.partial'
    with partial.open('w') as out, entries.open() as pending:
        head = json.dumps({**manifest, 'rides': []}, indent=2)
        tail = '"rides": []\n}'
        if not head.endswith(tail):
            raise AssertionError('manifest layout')
        out.write(head[: -len(tail)] + '"rides": [')
        for number, line in enumerate(pending):
            body = json.dumps(json.loads(line), indent=2).replace('\n', '\n    ')
            out.write((',' if number else '') + '\n    ' + body)
        out.write('\n  ]\n}\n' if count else ']\n}\n')
    partial.replace(output / 'manifest.json')
    entries.unlink()
    return {**manifest, 'ride_count': count}


def fit_command(canonical, output, start_utc_ms=None):
    """The firmware's own FIT encoder, built for the host (`apps/vana/src/bin/ride-fit.rs`)."""
    command = ['cargo', 'run', '--quiet', '--release', '--locked', '-p', 'vana', '--bin', 'ride-fit', '--']
    # Cargo runs from the workspace root, so relative paths would move.
    command += [str(Path(canonical).resolve()), str(Path(output).resolve())]
    if start_utc_ms is not None:
        command += ['--start-utc-ms', str(start_utc_ms)]
    return command


def write_fit(output, canonical, start_utc_ms=None, run=subprocess.run):
    """Convert every ride in `canonical` to `ride-<id>.fit`; list the results in `fit.json`."""
    result = run(fit_command(canonical, output, start_utc_ms), cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(f'FIT conversion failed: {result.stderr.strip()}')
    rides = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    (Path(output) / 'fit.json').write_text(
        json.dumps({'raw_file': Path(canonical).name, 'rides': rides}, indent=2) + '\n'
    )
    return rides


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_port_argument(parser)
    parser.add_argument('--output', type=Path, default=ROOT / '.local/exports' / str(time.time_ns()))
    parser.add_argument('--medium', choices=MEDIA, default='internal', help='ride journal to read (default internal)')
    parser.add_argument('--format', choices=('raw', 'fit'), default='raw', help='fit: also write FIT activities')
    parser.add_argument('--from', dest='source', type=Path, help='convert an earlier export instead of the device')
    parser.add_argument('--start-utc-ms', type=int, help='FIT start time for rides recorded without a clock')
    args = parser.parse_args()
    if args.source is not None:
        if args.format != 'fit':
            parser.error('--from needs --format fit')
        rides = write_fit(args.source, args.source / 'ride-slots.bin', args.start_utc_ms)
        print(json.dumps(rides, indent=2))
        print(f'FIT files are listed in {args.source / "fit.json"}')
        return
    summary = export(selected_port(parser, args.port), args.output, args.medium)
    if args.format == 'fit':
        rides = write_fit(args.output, args.output / 'ride-slots.bin', args.start_utc_ms)
        written = sum(ride['fit'] is not None for ride in rides)
        print(f'{written} FIT files are listed in {args.output / "fit.json"}')
    keep = ('medium', 'store_id', 'generation', 'upper_bound', 'status', 'raw_sha256', 'ride_count')
    print(json.dumps({key: summary[key] for key in keep if key in summary}, indent=2))
    print(f'Rides and their files are listed in {args.output / "manifest.json"}')
    print(f'Private export: {args.output}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        raise SystemExit(str(error)) from None
