"""Host-only checks for image validation and OTA selection; never opens USB."""
import hashlib
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zlib
import device


def image():
    header = bytearray(24)
    header[0], header[1], header[12], header[23] = 0xE9, 1, 9, 1
    data = header + struct.pack("<II", 0x40374000, 16) + bytes(range(16))
    checksum = 0xEF
    for value in range(16):
        checksum ^= value
    data += bytes(15) + bytes([checksum])
    return bytes(data) + hashlib.sha256(data).digest()


def metadata():
    data = bytearray(b"\xff" * 0x8000)
    layout = [(1, 2, 0x9000, 0x4000), (1, 0, 0xD000, 0x2000),
              (1, 1, 0xF000, 0x1000), (1, 3, 0x10000, 0x10000),
              (0, 0x10, 0x20000, 0x73A000), (0, 0x11, 0x760000, 0x73A000)]
    for i, (kind, subtype, offset, size) in enumerate(layout):
        data[i * 32:(i + 1) * 32] = struct.pack("<HBBII16sI", 0x50AA, kind, subtype, offset, size, b"test", 0)
    data[192:224] = b"\xeb\xeb" + b"\xff" * 14 + hashlib.md5(data[:192]).digest()
    for offset, seq in [(0x5000, 3), (0x6000, 2)]:
        struct.pack_into("<I", data, offset, seq)
        struct.pack_into("<I", data, offset + 28, zlib.crc32(data[offset:offset + 4], 0xFFFFFFFF))
    return bytes(data)


class Formats(unittest.TestCase):
    def test_valid_image_and_extent(self):
        good = image()
        self.assertEqual(device.image_length(good + b"\xff" * 32), len(good))

    def test_corrupt_image_is_rejected(self):
        for offset in [0, 12, 35, -1]:
            bad = bytearray(image())
            bad[offset] ^= 1
            with self.assertRaises(RuntimeError):
                device.image_length(bad)

    def test_truncated_image_is_rejected(self):
        for length in [0, 23, 31, 40, len(image()) - 1]:
            with self.assertRaises(RuntimeError):
                device.image_length(image()[:length])

    def test_partition_table_digest_is_required(self):
        good = metadata()
        device.partitions(good)
        bad = bytearray(good)
        bad[20] ^= 1
        with self.assertRaises(RuntimeError):
            device.partitions(bad)

    def test_different_partition_layout_is_rejected(self):
        bad = bytearray(metadata())
        struct.pack_into("<I", bad, 4 * 32 + 4, 0x30000)
        bad[208:224] = hashlib.md5(bad[:192]).digest()
        with self.assertRaises(RuntimeError):
            device.partitions(bad)

    def test_invalid_record_is_excluded(self):
        bad = bytearray(metadata())
        struct.pack_into("<I", bad, 0x5000 + 24, 3)
        self.assertEqual(device.records(bad), [(2, 0x6000)])

    def test_selection_changes_only_one_sector_and_can_return_to_stock(self):
        class Fake(device.Device):
            def __init__(self):
                self.memory = metadata()
                self.writes = []
                self.reboots = 0

            def write(self, address, path):
                self.writes.append(address)
                block = path.read_bytes()
                self.memory = self.memory[:address - 0x8000] + block + self.memory[address - 0x8000 + len(block):]

            def read(self, address, size, name):
                return self.memory, ""

            def reboot(self):
                self.reboots += 1

        fake = Fake()
        original = fake.memory
        with tempfile.TemporaryDirectory() as temp, patch.object(device, "LOCAL", Path(temp)):
            for target in [1, 0, 1, 1, 0]:
                fake.select(fake.memory, target)
                self.assertEqual((max(device.records(fake.memory))[0] - 1) % 2, target)
                self.assertEqual(fake.memory[:0x5000], original[:0x5000])
                self.assertEqual(fake.memory[0x7000:], original[0x7000:])
        self.assertEqual(fake.reboots, 5)
        self.assertTrue(all(address in (0xD000, 0xE000) for address in fake.writes))


if __name__ == "__main__":
    unittest.main()
