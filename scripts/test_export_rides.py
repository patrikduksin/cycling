import tempfile
import json
import unittest
from pathlib import Path
import xml.etree.ElementTree as ET
import zlib

from export_rides import (COMMIT, SLOT_SIZE, decode_slot, download_prefix,
                          read_slot, rides_from_slots, write_gpx)

from usb import terminal_reply

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
        data[at:at + 8] = sample['active_ms'].to_bytes(8, 'little')
        flags = 0
        if sample.get('utc_ms') is not None:
            flags |= 1
            data[at + 8:at + 16] = sample['utc_ms'].to_bytes(8, 'little')
        if sample.get('location') is not None:
            flags |= 2
            lat, lon = sample['location']
            data[at + 20:at + 24] = lat.to_bytes(4, 'little', signed=True)
            data[at + 24:at + 28] = lon.to_bytes(4, 'little', signed=True)
        if sample.get('speed') is not None:
            flags |= 4 | 0x80000000
            data[at + 28:at + 32] = sample['speed'].to_bytes(4, 'little')
        if sample.get('heart') is not None:
            flags |= 8
            data[at + 32:at + 34] = sample['heart'].to_bytes(2, 'little')
        if sample.get('cadence') is not None:
            flags |= 16
            data[at + 34:at + 36] = sample['cadence'].to_bytes(2, 'little')
        if sample.get('battery') is not None:
            flags |= 32
            data[at + 36] = sample['battery']
        data[at + 16:at + 20] = flags.to_bytes(4, 'little')
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


class RideExportTests(unittest.TestCase):
    def fixture(self):
        first = {'active_ms': 1000, 'utc_ms': 1_700_000_001_000,
                 'location': (100000000, 1800000000), 'heart': 72, 'battery': 90}
        missing = {'active_ms': 2000, 'utc_ms': None, 'location': None}
        later = {'active_ms': 3000, 'utc_ms': 1_699_999_999_000,
                 'location': (100000010, 200000010), 'cadence': 615}
        return [slot(1, 2, 7, 0, 0), slot(2, 2, 7, 1, 2000, [first, missing]),
                slot(3, 2, 7, 2, 2000), slot(4, 2, 7, 3, 2000),
                slot(2, 2, 7, 4, 3000, [later]), slot(6, 2, 7, 5, 3000)]

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
        damaged = bytearray(self.fixture()[1]); damaged[40] ^= 1
        rides, invalid = rides_from_slots([self.fixture()[0], bytes(damaged), self.fixture()[-1]])
        self.assertEqual(invalid, [1])
        self.assertTrue(rides[0]['gap'])
        unsupported = bytearray(self.fixture()[0]); unsupported[4] = 2
        with self.assertRaises(ValueError):
            decode_slot(bytes(unsupported))
        with self.assertRaises(ValueError):
            rides_from_slots([self.fixture()[0], self.fixture()[0]])
        with tempfile.TemporaryDirectory() as directory:
            no_location = {'ride_id': 1, 'source': 'demo', 'segments': []}
            self.assertEqual(write_gpx(no_location, Path(directory) / 'none.gpx'),
                             'no valid location samples')

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

    def test_interrupted_download_is_partial_and_retry_restarts_exactly(self):
        slots = self.fixture()[:3]
        with tempfile.TemporaryDirectory() as directory:
            partial = Path(directory) / 'raw.partial'
            with self.assertRaises(TimeoutError):
                download_prefix(FakeConnection(slots, fail_at=1), partial, len(slots))
            self.assertEqual(partial.read_bytes(), slots[0])
            download_prefix(FakeConnection(slots), partial, len(slots))
            self.assertEqual(partial.read_bytes(), b''.join(slots))

    def test_forward_gap_keeps_later_records_and_splits_unknown_source(self):
        sample = {'active_ms': 3000, 'utc_ms': 100, 'location': (1, 2)}
        slots = [slot(1, 2, 1, 0, 0), slot(2, 1, 9, 1, 3000, [sample]),
                 slot(2, 2, 1, 3, 3000, [sample]), slot(5, 2, 1, 4, 3000)]
        ride = rides_from_slots(slots)[0][0]
        self.assertTrue(ride['gap'])
        self.assertEqual(ride['terminal'], 'saved')
        self.assertEqual(len(ride['samples']), 1)

    def test_parser_fragmentation_ids_index_and_crc_are_strict(self):
        reply = b'noise\n' + b''.join(json.dumps(dict(type='reply', id=index, status='OK', data='INFO 1 256 2 ready')).encode() + b'\n' for index in [3, 4])
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
                return dict(id=1, status='OK', data=f'SLOT {self.index} {(zlib.crc32(raw) if self.crc is None else self.crc):08x} {raw.hex()}')
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
        slots = [slot(1, 2, 1, 0, 0), slot(2, 2, 1, 1, 3000, points),
                 slot(5, 2, 1, 2, 3000)]
        ride = rides_from_slots(slots)[0][0]
        self.assertEqual([len(segment) for segment in ride['segments']], [2, 1])
        ride['segments'][0][0]['utc_ms'] = (1 << 64) - 1
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(write_gpx(ride, Path(directory) / 'bad.gpx'),
                             'UTC timestamp outside GPX host range')


if __name__ == '__main__':
    unittest.main()
