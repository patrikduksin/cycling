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

    def test_link_terminal_and_foreign_records(self):
        data = slot(4, 1)
        data[24:28] = bytes([4, 0, 0, 0])
        struct.pack_into('<QIII', data, 28, 1500, 3, 8, 2)
        link = decode_slot(commit(data))
        self.assertEqual((link['link'], link['dropped_packets'], link['dropped_links']), ('disconnected', 8, 2))
        data = slot(2, 0)
        struct.pack_into('<II', data, 24, 8, 2)
        self.assertEqual(decode_slot(commit(data))['kind'], 'stopped')
        self.assertIsNone(decode_slot(b'RIDE' + bytes(252)))
        for kind, count in [(1, 0), (1, 9), (2, 1), (4, 0), (8, 0)]:
            with self.assertRaises(ValueError):
                decode_slot(slot(kind, count))


if __name__ == '__main__':
    unittest.main()
