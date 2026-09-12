"""Read the immutable ride-slot prefix over USB and create JSON/GPX exports."""
import argparse
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import select
import time
import xml.etree.ElementTree as ET
import zlib

from logs import ROOT, open_no_reset

SLOT_SIZE = 256
COMMIT = 0x434F4D54
MAX_SLOTS = 4096


class ExportConnection:
    def __init__(self, port, log):
        self.port, self.log_path = port, Path(log)
        self.fd = self.lock = self.log = None
        self.pending = bytearray()
        self.request_id = 0
        self.boot = [None]

    def __enter__(self):
        try:
            self.lock = (ROOT / '.local/usb.lock').open('w')
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.fd = open_no_reset(self.port)
            self.log = self.log_path.open('wb')
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_):
        fd, log, lock = self.fd, self.log, self.lock
        self.fd = self.log = self.lock = None
        try:
            if fd is not None:
                os.close(fd)
        finally:
            try:
                if log is not None:
                    log.close()
            finally:
                if lock is not None:
                    lock.close()

    def terminal_command(self, command, timeout=8):
        """Send once. The caller decides how to handle an uncertain mutation."""
        self.request_id += 1
        request = f'CMD {self.request_id} {command}\n'.encode()
        if os.write(self.fd, request) != len(request):
            raise RuntimeError('short USB command write; outcome may be uncertain')
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not select.select([self.fd], [], [], min(.2, max(0, deadline - time.monotonic())))[0]:
                continue
            data = os.read(self.fd, 65536)
            if not data:
                raise RuntimeError('device disconnected; command outcome may be uncertain')
            self.log.write(data)
            self.log.flush()
            reply = terminal_reply(self.pending, data, self.request_id, self.boot)
            if reply is not None:
                return reply
        raise TimeoutError(f'no terminal response for {command}')

    def command(self, command, timeout=8):
        reply = self.terminal_command(f'EXPORT {command}', timeout)
        if reply['status'] != 'OK':
            raise RuntimeError(f'export rejected: {reply["status"]}')
        return ['CYCLING_EXPORT', str(reply['id']), *reply['data'].split()]


def terminal_reply(pending, data, expected_id, boot=None):
    pending.extend(data)
    while b'\n' in pending:
        line, _, remainder = pending.partition(b'\n')
        pending[:] = remainder
        if len(line) > 4096:
            raise ValueError('oversized USB terminal line')
        if b'CYCLING_BOOT ' in line:
            raise RuntimeError('device rebooted during command')
        try:
            reply = json.loads(line)
        except (UnicodeDecodeError, ValueError):
            continue
        if not isinstance(reply, dict):
            continue
        if reply.get('type') == 'log':
            # The first observed integer token establishes this attachment's
            # identity, even when earlier startup text was unstructured. Reply
            # priority can also delay startup logs until after the first reply.
            if type(reply.get('boot')) is int:
                if boot is not None:
                    if boot[0] is not None and boot[0] != reply['boot']:
                        raise RuntimeError('device rebooted during command')
                    boot[0] = reply['boot']
            elif reply.get('component') == 'boot':
                raise RuntimeError('boot record has no identity; command outcome is uncertain')
            continue
        if reply.get('type') != 'reply':
            continue
        # The firmware discards the id when line parsing fails. This connection
        # permits only one outstanding command, so its documented id-zero parse
        # rejection belongs to that command. Never accept id-zero success or an
        # unrelated numbered reply as its completion.
        parse_rejection = (type(reply.get('id')) is int and reply['id'] == 0
                           and reply.get('status') in ('INVALID', 'OVERLONG'))
        if reply.get('id') != expected_id and not parse_rejection:
            continue
        if (type(reply.get('id')) is not int
                or not isinstance(reply.get('status'), str)
                or not isinstance(reply.get('data'), str)):
            raise ValueError('malformed terminal reply')
        return reply
    if len(pending) > 4096:
        raise ValueError('oversized USB terminal line')
    return None


def export_reply(pending, data, expected_id):
    """Export payload compatibility for callers; transport is ordinary terminal JSON."""
    reply = terminal_reply(pending, data, expected_id)
    if reply is None:
        return None
    if reply['status'] != 'OK':
        return ['CYCLING_EXPORT', str(reply['id']), 'ERROR', reply['status']]
    return ['CYCLING_EXPORT', str(reply['id']), *reply['data'].split()]


def info(connection):
    parts = connection.command('INFO')
    if len(parts) != 7 or parts[2] != 'INFO':
        raise ValueError('malformed export info')
    version, slot_size, upper = map(int, parts[3:6])
    if version != 1 or slot_size != SLOT_SIZE or not 0 <= upper <= MAX_SLOTS:
        raise ValueError('unsupported or out-of-bounds export info')
    return {'version': version, 'slot_size': slot_size, 'upper_bound': upper, 'status': parts[6]}


def read_slot(connection, index):
    parts = connection.command(f'SLOT {index}')
    if len(parts) != 6 or parts[2] != 'SLOT' or int(parts[3]) != index:
        raise ValueError('wrong slot response')
    try:
        data = bytes.fromhex(parts[5])
        expected = int(parts[4], 16)
    except ValueError as error:
        raise ValueError('malformed slot response') from error
    if len(data) != SLOT_SIZE or zlib.crc32(data) != expected:
        raise ValueError('short or corrupt slot response')
    return data


def download_prefix(connection, partial, upper):
    with Path(partial).open('wb') as raw:
        for index in range(upper):
            raw.write(read_slot(connection, index))
            raw.flush()


def u16(data, at):
    return int.from_bytes(data[at:at + 2], 'little')


def u32(data, at):
    return int.from_bytes(data[at:at + 4], 'little')


def u64(data, at):
    return int.from_bytes(data[at:at + 8], 'little')


def decode_slot(data):
    if len(data) != SLOT_SIZE or data == b'\xff' * SLOT_SIZE:
        return None
    count = data[6]
    if (data[:4] != b'RIDE' or data[4] != 1 or data[5] not in range(1, 8)
            or data[7] not in (1, 2) or count > 4 or u16(data, 24) != count * 48
            or data[26:28] != b'\0\0' or u32(data, 252) != COMMIT):
        raise ValueError('invalid ride record header')
    checked = bytearray(data[:252])
    stored_crc = u32(checked, 28)
    checked[28:32] = b'\0' * 4
    if zlib.crc32(checked) != stored_crc or (data[5] == 2) != (count > 0):
        raise ValueError('invalid ride record checksum or count')
    samples = []
    for index in range(count):
        value = data[32 + index * 48:80 + index * 48]
        flags = u32(value, 16)
        if flags & ~0x8000003f or flags & 4 and not flags & 0x80000000:
            raise ValueError('invalid sample flags')
        battery = value[36] if flags & 32 else None
        if battery is not None and battery > 100:
            raise ValueError('invalid battery value')
        signed = lambda at: int.from_bytes(value[at:at + 4], 'little', signed=True)
        samples.append({
            'active_ms': u64(value, 0),
            'utc_ms': u64(value, 8) if flags & 1 else None,
            'latitude_e7': signed(20) if flags & 2 else None,
            'longitude_e7': signed(24) if flags & 2 else None,
            'demo_speed_mm_s': u32(value, 28) if flags & 4 else None,
            'heart_bpm': u16(value, 32) if flags & 8 else None,
            'cadence_tenths': u16(value, 34) if flags & 16 else None,
            'battery_percent': battery,
        })
    active_ms = u64(data, 16)
    if samples and samples[-1]['active_ms'] != active_ms:
        raise ValueError('sample/header duration mismatch')
    return {'kind': data[5], 'source': 'demo' if data[7] == 1 else 'live',
            'ride_id': u32(data, 8), 'sequence': u32(data, 12),
            'active_ms': active_ms, 'samples': samples}


def rides_from_slots(slots):
    rides, current, seen = [], None, set()
    invalid = []
    for index, raw in enumerate(slots):
        try:
            entry = decode_slot(raw)
        except ValueError:
            invalid.append(index)
            if current:
                current['gap'] = True
                current['segments'].append([])
            continue
        if entry is None:
            if current:
                current['gap'] = True
                current['segments'].append([])
            continue
        entry['slot'] = index
        if entry['kind'] == 1:
            if entry['ride_id'] in seen:
                raise ValueError(f'duplicate ride id {entry["ride_id"]}')
            if current:
                current['terminal'] = 'interrupted'
                current['gap'] = True
                rides.append(current)
            seen.add(entry['ride_id'])
            current = {'ride_id': entry['ride_id'], 'source': entry['source'],
                       'start_slot': index, 'end_slot': None, 'terminal': None,
                       'gap': False, 'records': [index], 'samples': [], 'segments': [[]],
                       'next_sequence': entry['sequence'] + 1, 'active_ms': 0,
                       '_last_utc': None}
            continue
        if not current:
            continue
        if entry['ride_id'] != current['ride_id'] or entry['source'] != current['source']:
            current['gap'] = True
            if current['segments'][-1]:
                current['segments'].append([])
            continue
        current['records'].append(index)
        if entry['sequence'] != current['next_sequence']:
            current['gap'] = True
            if current['segments'][-1]:
                current['segments'].append([])
            if entry['sequence'] < current['next_sequence']:
                continue
        current['next_sequence'] = entry['sequence'] + 1
        current['active_ms'] = entry['active_ms']
        if entry['kind'] == 2:
            for sample in entry['samples']:
                current['samples'].append(sample)
                segment = current['segments'][-1]
                missing = sample['latitude_e7'] is None
                invalid_location = not missing and not (
                    -900_000_000 <= sample['latitude_e7'] <= 900_000_000
                    and -1_800_000_000 <= sample['longitude_e7'] <= 1_800_000_000)
                backwards = (sample['utc_ms'] is not None and current['_last_utc'] is not None
                             and sample['utc_ms'] < current['_last_utc'])
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
        elif entry['kind'] in (5, 6, 7):
            current['terminal'] = {5: 'saved', 6: 'recovered', 7: 'full'}[entry['kind']]
            current['end_slot'] = index
            current['segments'] = [segment for segment in current['segments'] if segment]
            rides.append(current)
            current = None
    if current:
        current['terminal'] = 'interrupted'
        current['gap'] = True
        current['segments'] = [segment for segment in current['segments'] if segment]
        rides.append(current)
    return rides, invalid


def write_gpx(ride, path):
    segments = ride['segments']
    if not segments:
        return 'no valid location samples'
    for segment in segments:
        for sample in segment:
            if sample['utc_ms'] is not None:
                try:
                    seconds, milliseconds = divmod(sample['utc_ms'], 1000)
                    dt.datetime(1970, 1, 1, tzinfo=dt.UTC) + dt.timedelta(
                        seconds=seconds, milliseconds=milliseconds)
                except (OverflowError, ValueError):
                    return 'UTC timestamp outside GPX host range'
    ns = 'http://www.topografix.com/GPX/1/1'
    ET.register_namespace('', ns)
    root = ET.Element(f'{{{ns}}}gpx', version='1.1', creator='cycling')
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
            if sample['utc_ms'] is not None:
                seconds, milliseconds = divmod(sample['utc_ms'], 1000)
                stamp = dt.datetime(1970, 1, 1, tzinfo=dt.UTC) + dt.timedelta(
                    seconds=seconds, milliseconds=milliseconds)
                ET.SubElement(point, f'{{{ns}}}time').text = stamp.isoformat(timespec='milliseconds').replace('+00:00', 'Z')
            points += 1
    if points == 0:
        return 'no valid location samples'
    ET.ElementTree(root).write(path, encoding='utf-8', xml_declaration=True)
    return None


def export(port, output):
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    partial = output / 'ride-slots.bin.partial'
    with ExportConnection(port, output / 'usb.log') as connection:
        metadata = info(connection)
        download_prefix(connection, partial, metadata['upper_bound'])
        if info(connection) != metadata:
            raise RuntimeError('ride prefix changed during export')
    canonical = output / 'ride-slots.bin'
    partial.replace(canonical)
    data = canonical.read_bytes()
    slots = [data[index:index + SLOT_SIZE] for index in range(0, len(data), SLOT_SIZE)]
    rides, invalid = rides_from_slots(slots)
    summaries = []
    for ride in rides:
        public = {key: value for key, value in ride.items()
                  if key != 'segments' and not key.startswith('_')}
        json_path = output / f'ride-{ride["ride_id"]}.json'
        json_path.write_text(json.dumps(public, indent=2) + '\n')
        gpx_path = output / f'ride-{ride["ride_id"]}.gpx'
        error = write_gpx(ride, gpx_path)
        summaries.append({'ride_id': ride['ride_id'], 'source': ride['source'],
                          'terminal': ride['terminal'], 'samples': len(ride['samples']),
                          'gpx': None if error else gpx_path.name, 'gpx_error': error,
                          'start_identity': hashlib.sha256(slots[ride['start_slot']]).hexdigest()})
    manifest = {**metadata, 'raw_file': canonical.name,
                'raw_sha256': hashlib.sha256(data).hexdigest(),
                'invalid_slots': invalid, 'rides': summaries}
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--output', type=Path, default=ROOT / '.local/exports' / str(time.time_ns()))
    args = parser.parse_args()
    manifest = export(args.port, args.output)
    print(json.dumps(manifest, indent=2))
    print(f'Private export: {args.output}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        raise SystemExit(str(error)) from None
