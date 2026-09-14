import struct
import contextlib
import io
import tempfile
from pathlib import Path
from unittest.mock import patch

import ant_capture
import unittest
import zlib

from ant_capture import decode_slot


def slot(kind=1, count=1):
    data = bytearray([255] * 256)
    data[:8] = b'ANT1' + bytes([1, kind, count, 0])
    struct.pack_into('<IIQ', data, 8, 18, 0, 1000)
    struct.pack_into('<BBHQII8s', data, 24, 40, 5, 1234, 990, 2, 3, b'\x30\x01\x00\x04\x00\x00\x02\x00')
    return commit(data)


def commit(data):
    struct.pack_into('<I', data, 248, zlib.crc32(data[:248]))
    data[252:] = b'TMOC'
    return data


class AntCaptureTests(unittest.TestCase):
    def test_packet_fields_and_corrupt_or_uncommitted_slots(self):
        data = slot()
        packet = decode_slot(data)['packets'][0]
        self.assertEqual(packet['received_ms'], 990)
        self.assertEqual(packet['device_number'], 1234)
        self.assertEqual(packet['payload_hex'], '3001000400000200')
        data[30] ^= 1
        with self.assertRaises(ValueError):
            decode_slot(data)
        data = slot()
        data[252:] = b'\xff' * 4
        with self.assertRaises(ValueError):
            decode_slot(data)

    def test_typed_links_preserve_legacy_decoder_and_reject_invalid_layout(self):
        for device_type in (40, 120, 11):
            data = slot(5, 1)
            data[24:28] = bytes([device_type, 2, 0, 0])
            struct.pack_into('<QIII', data, 28, 1600, 4, 7, 1)
            record = decode_slot(commit(data))
            self.assertEqual(record['device_type'], device_type)
            self.assertEqual(record['link'], 'connected')
            self.assertEqual(record['received_ms'], 1600)
            self.assertEqual(record['generation'], 4)
            self.assertEqual(record['dropped_packets'], 7)
            self.assertEqual(record['dropped_links'], 1)
            for offset, value in ((25, 7), (26, 1), (27, 1)):
                invalid = bytearray(data)
                invalid[offset] = value
                with self.assertRaises(ValueError):
                    decode_slot(commit(invalid))

    def test_simultaneous_packets_retain_each_sensor_identity(self):
        data = slot(1, 3)
        identities = [(40, 5, 1234), (120, 1, 5678), (11, 2, 9012)]
        for index, identity in enumerate(identities):
            struct.pack_into('<BBHQII8s', data, 24 + 28 * index,
                             *identity, 990, index, 0, bytes([index] * 8))
        packets = decode_slot(commit(data))['packets']
        self.assertEqual([(p['device_type'], p['transmission_type'], p['device_number'])
                          for p in packets], identities)
        self.assertEqual([p['received_ms'] for p in packets], [990] * 3)

    def test_positions_with_fix_stale_and_no_source(self):
        for flags, observed, latitude, longitude in (
                (3, 900, -334567890, -706543210), (1, 100, 0, 0), (0, 0, 0, 0)):
            data = slot(6, 1)
            data[24:28] = bytes([flags, 0, 0, 0])
            struct.pack_into('<QQiiI', data, 28, 1000, observed, latitude, longitude, 2)
            record = decode_slot(commit(data))
            self.assertEqual(record['kind'], 'position')
            self.assertEqual(record['fix_valid'], bool(flags & 2))
            self.assertEqual(record['latitude_e7'], latitude if flags & 2 else None)
            self.assertEqual(record['longitude_e7'], longitude if flags & 2 else None)
            self.assertEqual(record['source_age_ms'], 1000 - observed if flags & 1 else None)
            self.assertEqual(record['dropped_positions'], 2)
        for offset, value in ((24, 4), (25, 1), (36, 1), (44, 1)):
            invalid = bytearray(data)
            invalid[offset] = value
            with self.assertRaises(ValueError):
                decode_slot(commit(invalid))
        terminal = slot(2, 0)
        self.assertNotIn('dropped_positions', decode_slot(terminal))
        terminal[32:36] = b'GPS1'
        struct.pack_into('<I', terminal, 36, 3)
        self.assertEqual(decode_slot(commit(terminal))['dropped_positions'], 3)

    def environmental(self, flags=31):
        data = slot(7, 1)
        data[24:128] = bytes(104)
        data[24] = flags
        struct.pack_into('<QIII', data, 28, 1000, 3, 4, 5)
        if flags & 1:
            struct.pack_into('<QIh', data, 48, 900, 10_132_501, -1234)
        if flags & 2:
            struct.pack_into('<Qhhh', data, 64, 990, -32768, 32767, -1)
        if flags & 4:
            struct.pack_into('<Qhhh', data, 80, 980, 1, 2, 3)
        if flags & 8:
            struct.pack_into('<QBxH', data, 96, 800, 73, 4000)
        if flags & 16:
            struct.pack_into('<QB', data, 108, 800, 7)
        struct.pack_into('<II', data, 120, 6, 7)
        return commit(data)

    def test_environmental_units_signed_motion_missing_and_terminal_counts(self):
        record = decode_slot(self.environmental())
        self.assertEqual(record['pressure'], dict(received_ms=900, source_age_ms=100,
                                                pressure_centi_pa=10_132_501, temperature_centi_c=-1234))
        self.assertEqual(record['motion_1']['raw_axes'], [-32768, 32767, -1])
        self.assertEqual(record['power']['raw_status'], 7)
        self.assertEqual(record['battery']['interpreted_millivolts'], 4000)
        self.assertEqual(record['dropped_environment'], 5)
        missing = decode_slot(self.environmental(0))
        for source in ('pressure', 'motion_1', 'motion_2', 'battery', 'power'):
            self.assertIsNone(missing[source])
        for offset, value in [(24, 32), (25, 1), (62, 1), (78, 1), (94, 1),
                              (105, 1), (117, 1), (128, 0), (48, 1)]:
            data = self.environmental(0)
            data[offset] = value
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                decode_slot(commit(data))
        data = self.environmental()
        struct.pack_into('<Q', data, 48, 1001)
        with self.assertRaises(ValueError):
            decode_slot(commit(data))
        data = slot(2, 0)
        data[40:44] = b'ENV1'
        struct.pack_into('<I', data, 44, 17)
        self.assertEqual(decode_slot(commit(data))['dropped_environment'], 17)

    def test_export_preserves_torn_unknown_records_and_sequence_gaps(self):
        first = self.environmental()
        struct.pack_into('<II', first, 8, 0, 0)
        first = commit(first)
        torn = self.environmental()
        torn[252:] = b'\xff' * 4
        third = self.environmental(0)
        struct.pack_into('<II', third, 8, 0, 2)
        third = commit(third)
        unknown = slot(99, 0)
        media = [first, torn, third, unknown]
        commands = []
        class Connection:
            def __init__(self, *args): pass
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def terminal_command(self, command):
                commands.append(command)
                if command == 'FOUNDATION LOG INFO':
                    data = 'INFO 1 256 4 stopped'
                else:
                    index = int(command.split()[-1])
                    blob = media[index]
                    data = f'SLOT {index} {zlib.crc32(blob):08x} {blob.hex()}'
                return dict(status='OK', data=data)
        with tempfile.TemporaryDirectory() as directory, patch.object(ant_capture, 'UsbConnection', Connection):
            output = Path(directory) / 'capture'
            with contextlib.redirect_stdout(io.StringIO()):
                manifest = ant_capture.export('fake', output, 'FOUNDATION')
            self.assertEqual((output / 'prefix.bin').read_bytes(), b''.join(media))
            self.assertEqual(manifest['environmental_samples'], 2)
            self.assertEqual(len(manifest['invalid_ant_slots']), 2)
            self.assertEqual(len(manifest['sequence_gaps']), 1)
            self.assertTrue(all(command.startswith('FOUNDATION LOG ') for command in commands))

    def test_interrupted_export_retains_partial_and_never_claims_manifest(self):
        blob = self.environmental()
        class Connection:
            def __init__(self, *args): pass
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def terminal_command(self, command):
                if command.endswith('INFO'):
                    return dict(status='OK', data='INFO 1 256 2 stopped')
                if command.endswith('READ 0'):
                    return dict(status='OK', data=f'SLOT 0 {zlib.crc32(blob):08x} {blob.hex()}')
                raise TimeoutError('synthetic disconnect')
        with tempfile.TemporaryDirectory() as directory, patch.object(ant_capture, 'UsbConnection', Connection):
            output = Path(directory) / 'capture'
            with self.assertRaises(TimeoutError):
                ant_capture.export('fake', output, 'FOUNDATION')
            self.assertEqual((output / 'prefix.partial').read_bytes(), blob)
            self.assertFalse((output / 'prefix.bin').exists())
            self.assertFalse((output / 'manifest.json').exists())

    def test_link_terminal_and_foreign_records(self):
        data = slot(4, 1)
        data[24:28] = bytes([4, 0, 0, 0])
        struct.pack_into('<QIII', data, 28, 1500, 3, 8, 2)
        link = decode_slot(commit(data))
        self.assertNotIn('device_type', link)
        self.assertEqual((link['link'], link['dropped_packets'], link['dropped_links']), ('disconnected', 8, 2))
        data = slot(2, 0)
        struct.pack_into('<II', data, 24, 8, 2)
        self.assertEqual(decode_slot(commit(data))['kind'], 'stopped')
        self.assertIsNone(decode_slot(b'RIDE' + bytes(252)))
        for kind, count in [(1, 0), (1, 9), (2, 1), (4, 0), (5, 0), (5, 2), (6, 0), (6, 2), (8, 0)]:
            with self.assertRaises(ValueError):
                decode_slot(slot(kind, count))


if __name__ == '__main__':
    unittest.main()
