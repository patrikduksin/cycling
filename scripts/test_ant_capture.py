import struct
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
        for kind, count in [(1, 0), (1, 9), (2, 1), (4, 0), (5, 0), (5, 2), (8, 0)]:
            with self.assertRaises(ValueError):
                decode_slot(slot(kind, count))


if __name__ == '__main__':
    unittest.main()
