import hashlib
import json
import tempfile
import tracemalloc
import unittest
import xml.etree.ElementTree as ET
import zlib
from pathlib import Path

from cycling_devtools.rides.export import (
    COMMIT,
    SLOT_SIZE,
    bulk_data_start,
    decode_slot,
    download_prefix,
    info,
    read_owned_sector,
    read_slot,
    rides_from_slots,
    write_exports,
    write_gpx,
)
from cycling_devtools.terminal.usb import terminal_reply


def slot(kind, source, ride_id, sequence, active_ms, samples=()):
    data = bytearray(b'\xff' * SLOT_SIZE)
    data[:4] = b'RIDE'
    data[4:8] = bytes([1, kind, len(samples), source])
    data[8:12] = ride_id.to_bytes(4, 'little')
    data[12:16] = sequence.to_bytes(4, 'little')
    data[16:24] = active_ms.to_bytes(8, 'little')
    data[24:26] = (len(samples) * 48).to_bytes(2, 'little')
    data[26:32] = b'\0' * 6
    for index, sample in enumerate(samples):
        at = 32 + index * 48
        data[at : at + 8] = sample['active_ms'].to_bytes(8, 'little')
        flags = 0
        if sample.get('utc_ms') is not None:
            flags |= 1
            data[at + 8 : at + 16] = sample['utc_ms'].to_bytes(8, 'little')
        if sample.get('location') is not None:
            flags |= 2
            lat, lon = sample['location']
            data[at + 20 : at + 24] = lat.to_bytes(4, 'little', signed=True)
            data[at + 24 : at + 28] = lon.to_bytes(4, 'little', signed=True)
        if sample.get('speed') is not None:
            flags |= 4 | 0x80000000
            data[at + 28 : at + 32] = sample['speed'].to_bytes(4, 'little')
        if sample.get('heart') is not None:
            flags |= 8
            data[at + 32 : at + 34] = sample['heart'].to_bytes(2, 'little')
        if sample.get('cadence') is not None:
            flags |= 16
            data[at + 34 : at + 36] = sample['cadence'].to_bytes(2, 'little')
        if sample.get('battery') is not None:
            flags |= 32
            data[at + 36] = sample['battery']
        if sample.get('altitude_m') is not None:
            data[at + 46 : at + 48] = round((sample['altitude_m'] + 500) * 5).to_bytes(2, 'little')
        if sample.get('lap') is not None:
            data[at + 37] = sample['lap']
        data[at + 16 : at + 20] = flags.to_bytes(4, 'little')
    data[28:32] = zlib.crc32(data[:252]).to_bytes(4, 'little')
    data[252:256] = COMMIT.to_bytes(4, 'little')
    return bytes(data)


class FakeConnection:
    def __init__(self, slots, fail_at=None):
        self.slots, self.fail_at = slots, fail_at

    def terminal_command(self, command):
        index = int(command.split()[2])
        if index == self.fail_at:
            raise TimeoutError('injected interruption')
        data = self.slots[index]
        return dict(id=index + 1, status='OK', data=f'SLOT {index} {zlib.crc32(data):08x} {data.hex()}')


class FakeBulk:
    """Bulk export replies: slot containers, one invalid, and raw owned sectors."""

    def __init__(self, slots, invalid=(), capacity=5_777_391):
        self.slots, self.invalid, self.capacity, self.commands = slots, set(invalid), capacity, []

    def terminal_command(self, command):
        self.commands.append(command)
        words = command.split()
        if words[:3] == ['EXPORT', 'BULK', 'INFO']:
            data = f'INFO 1 256 {len(self.slots)} ready 00000000000000ab 2 {self.capacity}'
        elif words[:3] == ['EXPORT', 'BULK', 'SLOT']:
            index = int(words[3])
            valid = index not in self.invalid
            data = self.slots[index] if valid else bytes(SLOT_SIZE)
            data = f'SLOT {index} {zlib.crc32(data):08x} {data.hex()} {"valid" if valid else "invalid"}'
        elif words == ['VOLUME', 'STATUS']:
            data = 'layout=volumes sectors=5777407 generation=1 unallocated=0 rides=2048+2097152:log'
        elif words[:3] == ['MMC', 'OWNED', 'READ']:
            sector, offset = int(words[3]), int(words[4])
            chunk = bytes([sector & 0xFF]) * 256 if offset else b'\xa5' * 256
            data = f'sector={sector} offset={offset} length=256 crc32={zlib.crc32(chunk):08x} data={chunk.hex()}'
        else:
            raise AssertionError(command)
        return dict(id=len(self.commands), status='OK', data=data)


class RideExportTests(unittest.TestCase):
    def fixture(self):
        first = {
            'active_ms': 1000,
            'utc_ms': 1_700_000_001_000,
            'location': (100000000, 1800000000),
            'heart': 72,
            'battery': 90,
            'altitude_m': 123.4,
        }
        missing = {'active_ms': 2000, 'utc_ms': None, 'location': None}
        later = {'active_ms': 3000, 'utc_ms': 1_699_999_999_000, 'location': (100000010, 200000010), 'cadence': 615}
        return [
            slot(1, 2, 7, 0, 0),
            slot(2, 2, 7, 1, 2000, [first, missing]),
            slot(3, 2, 7, 2, 2000),
            slot(4, 2, 7, 3, 2000),
            slot(2, 2, 7, 4, 3000, [later]),
            slot(6, 2, 7, 5, 3000),
        ]

    def test_workout_optional_fields_preserve_legacy_samples(self):
        data = bytearray(slot(2, 2, 1, 1, 1000, [{'active_ms': 1000}]))
        data[48:52] = (0x1C0).to_bytes(4, 'little')
        data[70:72] = (215).to_bytes(2, 'little')
        data[72:76] = (12345).to_bytes(4, 'little')
        data[76:78] = (-37).to_bytes(2, 'little', signed=True)
        data[28:32] = bytes(4)
        data[28:32] = zlib.crc32(data[:252]).to_bytes(4, 'little')
        sample = decode_slot(bytes(data))['samples'][0]
        self.assertEqual((sample['speed_mm_s'], sample['power_watts'], sample['gradient_tenths']), (12345, 215, -37))
        old = decode_slot(self.fixture()[1])['samples'][1]
        self.assertIsNone(old['speed_mm_s'])
        self.assertIsNone(old['power_watts'])
        self.assertIsNone(old['gradient_tenths'])
        # Samples written before altitude was kept have erased bytes 46..48.
        self.assertIsNone(decode_slot(self.fixture()[1])['samples'][1]['altitude_m'])
        self.assertEqual(decode_slot(self.fixture()[1])['samples'][0]['altitude_m'], 123.4)

    def test_lap_marks_list_the_laps_and_mark_them_on_the_gpx(self):
        def at(second, lap=None):
            location = (-333_646_900 + second * 100, -705_155_800)
            return {
                'active_ms': second * 1000,
                'utc_ms': 1_789_000_000_000 + second * 1000,
                'location': location,
                'lap': lap,
            }

        slots = [
            slot(1, 2, 3, 0, 0),
            slot(2, 2, 3, 1, 4000, [at(1, 0), at(2), at(3, 1), at(4)]),
            slot(2, 2, 3, 2, 6000, [at(5, 2), at(6, 0x42)]),
            slot(5, 2, 3, 3, 6000),
        ]
        rides, invalid = rides_from_slots(slots)
        self.assertEqual(invalid, [], 'the marks change no checked byte')
        self.assertEqual(len(rides[0]['samples']), 6)
        laps = rides[0]['laps']
        self.assertEqual(
            [(lap['lap'], lap['started_by'], lap['active_ms']) for lap in laps],
            [(1, 'ride', 1000), (2, 'manual', 3000), (3, 'distance', 5000)],
        )
        self.assertIsNone(rides[0]['samples'][5]['lap'], 'an unknown mark reads as none')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'ride.gpx'
            self.assertIsNone(write_gpx(rides[0], path))
            ns = '{http://www.topografix.com/GPX/1/1}'
            root = ET.parse(path).getroot()
            self.assertEqual([child.tag for child in root], [f'{ns}wpt', f'{ns}wpt', f'{ns}trk'], 'GPX 1.1 order')
            names = [
                (point.find(f'{ns}name').text, point.find(f'{ns}type').text, point.attrib['lat'])
                for point in root.findall(f'{ns}wpt')
            ]
            self.assertEqual(names, [('LAP 2', 'manual', '-33.3646600'), ('LAP 3', 'distance', '-33.3646400')])
            self.assertEqual(root.find(f'{ns}wpt/{ns}time').text, '2026-09-10T00:26:43.000Z')

    def test_a_ride_recorded_before_lap_marks_has_no_laps(self):
        rides, _ = rides_from_slots(self.fixture())
        self.assertIsNone(rides[0]['laps'])
        self.assertTrue(all(sample['lap'] is None for sample in rides[0]['samples']))

    def test_known_fixture_decodes_values_and_segments(self):
        rides, invalid = rides_from_slots(self.fixture())
        self.assertEqual(invalid, [])
        self.assertEqual(len(rides), 1)
        ride = rides[0]
        self.assertEqual((ride['ride_id'], ride['source'], ride['terminal']), (7, 'live', 'recovered'))
        self.assertEqual(len(ride['samples']), 3)
        self.assertEqual(ride['samples'][0]['heart_bpm'], 72)
        self.assertEqual(ride['samples'][2]['cadence_tenths'], 615)
        self.assertEqual([len(segment) for segment in ride['segments']], [1, 1])

    def test_corrupt_unsupported_duplicate_and_location_free_are_explicit(self):
        damaged = bytearray(self.fixture()[1])
        damaged[40] ^= 1
        rides, invalid = rides_from_slots([self.fixture()[0], bytes(damaged), self.fixture()[-1]])
        self.assertEqual(invalid, [1])
        self.assertTrue(rides[0]['gap'])
        unsupported = bytearray(self.fixture()[0])
        unsupported[4] = 2
        with self.assertRaises(ValueError):
            decode_slot(bytes(unsupported))
        with self.assertRaises(ValueError):
            rides_from_slots([self.fixture()[0], self.fixture()[0]])
        with tempfile.TemporaryDirectory() as directory:
            no_location = {'ride_id': 1, 'source': 'demo', 'segments': []}
            self.assertEqual(write_gpx(no_location, Path(directory) / 'none.gpx'), 'no valid location samples')

    def test_gpx_11_normalizes_positive_180_and_preserves_time(self):
        ride = rides_from_slots(self.fixture())[0][0]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'ride.gpx'
            self.assertIsNone(write_gpx(ride, path))
            root = ET.parse(path).getroot()
            self.assertEqual(root.attrib['version'], '1.1')
            self.assertEqual(root.attrib['creator'], 'cycling')
            points = root.findall('.//{http://www.topografix.com/GPX/1/1}trkpt')
            self.assertEqual(points[0].attrib['lon'], '-180.0000000')
            self.assertTrue(points[0].find('{http://www.topografix.com/GPX/1/1}time').text.endswith('Z'))
            ns = '{http://www.topografix.com/GPX/1/1}'
            self.assertEqual(points[0].find(f'{ns}ele').text, '123.4')
            self.assertEqual([child.tag for child in points[0]], [f'{ns}ele', f'{ns}time'])
            self.assertIsNone(points[1].find(f'{ns}ele'))

    def test_interrupted_download_is_partial_and_retry_restarts_exactly(self):
        slots = self.fixture()[:3]
        with tempfile.TemporaryDirectory() as directory:
            partial = Path(directory) / 'raw.partial'
            with self.assertRaises(TimeoutError):
                download_prefix(FakeConnection(slots, fail_at=1), partial, len(slots))
            self.assertEqual(partial.read_bytes(), slots[0])
            download_prefix(FakeConnection(slots), partial, len(slots))
            self.assertEqual(partial.read_bytes(), b''.join(slots))

    def test_bulk_export_reads_the_bulk_log_and_reports_invalid_containers(self):
        slots = self.fixture()
        connection = FakeBulk(slots, invalid={2})
        metadata = info(connection, 'bulk')
        self.assertEqual(metadata['upper_bound'], len(slots))
        self.assertEqual((metadata['store_id'], metadata['generation']), ('00000000000000ab', 2))
        with tempfile.TemporaryDirectory() as directory:
            partial = Path(directory) / 'slots.partial'
            self.assertEqual(download_prefix(connection, partial, len(slots), 'bulk'), [2])
            raw = partial.read_bytes()
        self.assertEqual(raw[2 * SLOT_SIZE : 3 * SLOT_SIZE], bytes(SLOT_SIZE))
        self.assertEqual(raw[:SLOT_SIZE], slots[0])
        self.assertTrue(all(command.startswith('EXPORT BULK ') for command in connection.commands))
        sector = read_owned_sector(connection, 18)
        self.assertEqual(sector, b'\xa5' * 256 + bytes([18]) * 256)
        self.assertEqual(connection.commands[-2:], ['MMC OWNED READ 18 0 256', 'MMC OWNED READ 18 256 256'])

    def test_bulk_export_info_is_bounded_by_capacity_and_strict(self):
        with self.assertRaisesRegex(ValueError, 'out-of-bounds'):
            info(FakeBulk(self.fixture(), capacity=2), 'bulk')

        class Internal(FakeBulk):
            def terminal_command(self, command):
                reply = super().terminal_command(command)
                reply['data'] = reply['data'].rsplit(' ', 3)[0]
                return reply

        with self.assertRaisesRegex(ValueError, 'malformed'):
            info(Internal(self.fixture()), 'bulk')
        with self.assertRaises(ValueError):
            read_slot(FakeBulk(self.fixture()), 0, 'removable')

    def test_forward_gap_keeps_later_records_and_splits_unknown_source(self):
        sample = {'active_ms': 3000, 'utc_ms': 100, 'location': (1, 2)}
        slots = [
            slot(1, 2, 1, 0, 0),
            slot(2, 1, 9, 1, 3000, [sample]),
            slot(2, 2, 1, 3, 3000, [sample]),
            slot(5, 2, 1, 4, 3000),
        ]
        ride = rides_from_slots(slots)[0][0]
        self.assertTrue(ride['gap'])
        self.assertEqual(ride['terminal'], 'saved')
        self.assertEqual(len(ride['samples']), 1)

    def test_parser_fragmentation_ids_index_and_crc_are_strict(self):
        reply = b'noise\n' + b''.join(
            json.dumps(dict(type='reply', id=index, status='OK', data='INFO 1 256 2 ready')).encode() + b'\n'
            for index in [3, 4]
        )
        for split in range(len(reply) + 1):
            pending = bytearray()
            first = terminal_reply(pending, reply[:split], 4)
            result = first or terminal_reply(pending, reply[split:], 4)
            self.assertEqual(result['data'].split()[0], 'INFO')
        with self.assertRaises(RuntimeError):
            terminal_reply(bytearray(), b'CYCLING_BOOT version=x\n', 1)
        raw = self.fixture()[0]

        class Reply:
            def __init__(self, index=0, crc=None):
                self.index, self.crc = index, crc

            def terminal_command(self, _):
                return dict(
                    id=1,
                    status='OK',
                    data=f'SLOT {self.index} {(zlib.crc32(raw) if self.crc is None else self.crc):08x} {raw.hex()}',
                )

        with self.assertRaises(ValueError):
            read_slot(Reply(index=1), 0)
        with self.assertRaises(ValueError):
            read_slot(Reply(crc=0), 0)

        class Short(Reply):
            def terminal_command(self, _):
                reply = super().terminal_command(_)
                reply['data'] = reply['data'][:-2]
                return reply

        with self.assertRaises(ValueError):
            read_slot(Short(), 0)

    def test_terminal_json_is_bounded_correlated_and_detects_reboots(self):
        def line(**values):
            return json.dumps(values).encode() + b'\n'

        pending = bytearray()
        data = line(type='reply', id=9, status='OK', data='wrong') + line(type='reply', id=10, status='BUSY', data='')
        self.assertEqual(terminal_reply(pending, data, 10)['status'], 'BUSY')
        with self.assertRaisesRegex(ValueError, 'malformed'):
            terminal_reply(bytearray(), line(type='reply', id=10, status='OK', data=3), 10)
        for data in [b'x' * 4097, b'x' * 4097 + b'\n']:
            with self.assertRaisesRegex(ValueError, 'oversized'):
                terminal_reply(bytearray(), data, 10)
        initial_boot = [None]
        self.assertIsNone(terminal_reply(bytearray(), line(type='log', boot=8, component='boot'), 10, initial_boot))
        self.assertEqual(initial_boot, [8])
        with self.assertRaisesRegex(RuntimeError, 'no identity'):
            terminal_reply(bytearray(), line(type='log', component='boot'), 10, initial_boot)
        boot = [None]
        self.assertIsNone(terminal_reply(bytearray(), line(type='log', boot=8, component='gps'), 10, boot))
        with self.assertRaisesRegex(RuntimeError, 'rebooted'):
            terminal_reply(bytearray(), line(type='log', boot=9, component='gps'), 10, boot)

    def test_delayed_same_boot_startup_log_does_not_reject_next_reply(self):
        def line(**values):
            return json.dumps(values).encode() + b'\n'

        first = line(type='log', boot=12, component='build', ms=1334)
        first += line(type='reply', id=1, status='OK', data='metadata', ms=1923)
        second = line(type='log', boot=12, component='boot', ms=1334)
        second += line(type='reply', id=2, status='OK', data='brightness=100', ms=1969)
        for split in range(len(first) + len(second) + 1):
            wire = first + second
            pending, boot = bytearray(), [None]
            reply = terminal_reply(pending, wire[:split], 1, boot)
            if reply is None:
                reply = terminal_reply(pending, wire[split:], 1, boot)
                remaining = b''
            else:
                remaining = wire[split:]
            self.assertEqual(reply['id'], 1)
            reply = terminal_reply(pending, remaining, 2, boot)
            self.assertEqual((reply['id'], reply['data']), (2, 'brightness=100'))
        with self.assertRaisesRegex(RuntimeError, 'rebooted'):
            terminal_reply(bytearray(), line(type='log', boot=13, component='boot'), 2, [12])

    def test_first_clean_boot_token_establishes_identity_after_ansi_startup(self):
        def line(**values):
            return json.dumps(values).encode() + b'\n'

        first = b'\x1b[31m' + line(type='log', boot=12, component='build')
        first += line(type='reply', id=1, status='OK', data='metadata')
        second = line(type='log', boot=12, component='boot', message='crash=controlled')
        second += line(type='reply', id=2, status='OK', data='ready')
        wire = first + second
        for split in range(len(wire) + 1):
            pending, boot = bytearray(), [None]
            reply = terminal_reply(pending, wire[:split], 1, boot)
            if reply is None:
                reply = terminal_reply(pending, wire[split:], 1, boot)
                remaining = b''
            else:
                remaining = wire[split:]
            self.assertEqual(reply['id'], 1)
            self.assertIsNone(boot[0])
            reply = terminal_reply(pending, remaining, 2, boot)
            self.assertEqual((reply['id'], boot[0]), (2, 12))
        for component in ['boot', 'position']:
            with self.assertRaisesRegex(RuntimeError, 'rebooted'):
                terminal_reply(bytearray(), line(type='log', boot=13, component=component), 3, boot)

    def test_single_outstanding_command_receives_id_zero_parse_rejection(self):
        for status in ['INVALID', 'OVERLONG']:
            wire = json.dumps(dict(type='reply', id=0, status=status, ms=123, data='')).encode() + b'\n'
            for split in range(len(wire) + 1):
                pending = bytearray()
                reply = terminal_reply(pending, wire[:split], 42)
                reply = reply or terminal_reply(pending, wire[split:], 42)
                self.assertEqual((reply['id'], reply['status'], reply['ms']), (0, status, 123))
        for status in ['OK', 'ACCEPTED', 'STATE']:
            wire = json.dumps(dict(type='reply', id=0, status=status, data='')).encode() + b'\n'
            self.assertIsNone(terminal_reply(bytearray(), wire, 42))
        wire = json.dumps(dict(type='reply', id=41, status='INVALID', data='')).encode() + b'\n'
        self.assertIsNone(terminal_reply(bytearray(), wire, 42))

    def test_backward_utc_across_untimed_point_and_overflow_are_explicit(self):
        points = [
            {'active_ms': 1000, 'utc_ms': 100, 'location': (1, 2)},
            {'active_ms': 2000, 'utc_ms': None, 'location': (2, 3)},
            {'active_ms': 3000, 'utc_ms': 90, 'location': (3, 4)},
        ]
        slots = [slot(1, 2, 1, 0, 0), slot(2, 2, 1, 1, 3000, points), slot(5, 2, 1, 2, 3000)]
        ride = rides_from_slots(slots)[0][0]
        self.assertEqual([len(segment) for segment in ride['segments']], [2, 1])
        ride['segments'][0][0]['utc_ms'] = (1 << 64) - 1
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(write_gpx(ride, Path(directory) / 'bad.gpx'), 'UTC timestamp outside GPX host range')

    def test_clock_sync_mid_ride_dates_earlier_samples_of_the_same_running_stretch(self):
        before = [{'active_ms': ms, 'utc_ms': None, 'location': (ms, ms)} for ms in (1000, 2000, 3000)]
        synced = {'active_ms': 4000, 'utc_ms': 1_700_000_004_250, 'location': (4000, 4000)}
        after_pause = [
            {'active_ms': 5000, 'utc_ms': None, 'location': (5000, 5000)},
            {'active_ms': 6000, 'utc_ms': 1_700_000_100_000, 'location': (6000, 6000)},
        ]
        slots = [
            slot(1, 2, 3, 0, 0),
            slot(2, 2, 3, 1, 4000, [*before, synced]),
            slot(3, 2, 3, 2, 4000),
            slot(4, 2, 3, 3, 4000),
            slot(2, 2, 3, 4, 6000, after_pause),
            slot(5, 2, 3, 5, 6000),
        ]
        ride = rides_from_slots(slots)[0][0]
        times = [sample['utc_ms'] for sample in ride['samples']]
        # Active time runs with the clock until the pause, so the first three
        # samples get exact times; the pause boundary is never crossed.
        self.assertEqual(times[:4], [1_700_000_001_250, 1_700_000_002_250, 1_700_000_003_250, 1_700_000_004_250])
        self.assertEqual([sample.get('utc_derived', False) for sample in ride['samples']][:4], [True] * 3 + [False])
        self.assertEqual(times[4], 1_700_000_099_000)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'ride.gpx'
            self.assertIsNone(write_gpx(ride, path))
            ns = {'g': 'http://www.topografix.com/GPX/1/1'}
            points = ET.parse(path).getroot().findall('.//g:trkpt', ns)
            self.assertEqual(len(points), 6)
            self.assertTrue(all(point.find('g:time', ns) is not None for point in points))

    def test_clock_sync_after_a_gap_does_not_date_samples_before_the_gap(self):
        untimed = [{'active_ms': 1000, 'utc_ms': None, 'location': (1, 1)}]
        timed = [{'active_ms': 9000, 'utc_ms': 1_700_000_009_000, 'location': (2, 2)}]
        slots = [slot(1, 2, 4, 0, 0), slot(2, 2, 4, 1, 1000, untimed), b'\0' * SLOT_SIZE, slot(2, 2, 4, 3, 9000, timed)]
        ride = rides_from_slots(slots)[0][0]
        self.assertIsNone(ride['samples'][0]['utc_ms'])
        self.assertNotIn('utc_derived', ride['samples'][0])


class LargeExportTests(unittest.TestCase):
    def test_a_large_prefix_exports_ride_by_ride_in_bounded_memory(self):
        # Many short rides: holding them all costs rides x samples, streaming only one ride.
        rides, batches = 60, 250
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            canonical = output / 'ride-slots.bin'
            with canonical.open('wb') as raw:
                for ride in range(1, rides + 1):
                    raw.write(slot(1, 2, ride, 0, 0))
                    for batch in range(1, batches + 1):
                        samples = [
                            {'active_ms': (batch - 1) * 4000 + step * 1000, 'heart': 120} for step in range(1, 5)
                        ]
                        raw.write(slot(2, 2, ride, batch, batch * 4000, samples))
                    raw.write(slot(5, 2, ride, batches + 1, batches * 4000))
            size = canonical.stat().st_size
            tracemalloc.start()
            try:
                summary = write_exports(output, canonical, {'version': 1, 'upper_bound': size // SLOT_SIZE})
                _, peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            # 60,000 samples: streaming peaks near 1.5 MiB, while merely holding
            # every sample takes over 12 MiB (tracemalloc, which makes this test slow).
            self.assertLess(peak, 4 * 2**20, f'{peak / 2**20:.1f} MiB peak')
            manifest = json.loads((output / 'manifest.json').read_text())
            self.assertEqual(summary['ride_count'], rides)
            self.assertEqual(manifest['raw_sha256'], hashlib.sha256(canonical.read_bytes()).hexdigest())
            self.assertEqual([entry['ride_id'] for entry in manifest['rides']], list(range(1, rides + 1)))
            self.assertTrue(all(entry['samples'] == batches * 4 for entry in manifest['rides']))
            self.assertEqual(manifest['invalid_slots'], [])
            self.assertFalse((output / 'manifest-rides.partial').exists())
            last = json.loads((output / f'ride-{rides}.json').read_text())
            self.assertEqual((last['terminal'], len(last['samples'])), ('saved', batches * 4))

    def test_an_empty_prefix_still_writes_a_valid_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            canonical = output / 'ride-slots.bin'
            canonical.write_bytes(b'')
            write_exports(output, canonical, {'version': 1})
            manifest = json.loads((output / 'manifest.json').read_text())
            self.assertEqual((manifest['rides'], manifest['raw_sha256']), ([], hashlib.sha256(b'').hexdigest()))


class BulkDataStartTest(unittest.TestCase):
    def start(self, status, data):
        class Reply:
            def terminal_command(self, command):
                assert command == 'VOLUME STATUS'
                return dict(id=1, status=status, data=data)

        return bulk_data_start(Reply())

    def test_rides_volume_offsets_record_zero(self):
        self.assertEqual(self.start('OK', 'layout=volumes rides=2048+2097152:log'), 2064)

    def test_firmware_without_volumes_keeps_the_legacy_start(self):
        self.assertEqual(self.start('INVALID', 'malformed command'), 16)

    def test_a_legacy_layout_under_volume_firmware_is_refused(self):
        with self.assertRaises(RuntimeError):
            self.start('OK', 'layout=legacy region=5777407')


if __name__ == '__main__':
    unittest.main()
