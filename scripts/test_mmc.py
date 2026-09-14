"""Synthetic media checks for bounded read-only eMMC investigation."""
import struct
import unittest
import zlib

import mmc


def put16(data, offset, value):
    struct.pack_into('<H', data, offset, value)


def put32(data, offset, value):
    struct.pack_into('<I', data, offset, value)


def put64(data, offset, value):
    struct.pack_into('<Q', data, offset, value)


def mbr(entries):
    data = bytearray(512)
    data[510:] = b'\x55\xaa'
    for index, (kind, start, length) in enumerate(entries):
        offset = 446 + index * 16
        data[offset + 4] = kind
        put32(data, offset + 8, start)
        put32(data, offset + 12, length)
    return bytes(data)


def fat32():
    boot = bytearray(512)
    boot[0] = 0xeb
    boot[510:] = b'\x55\xaa'
    put16(boot, 11, 512)
    boot[13] = 1
    put16(boot, 14, 32)
    boot[16] = 1
    put32(boot, 32, 70000)
    put32(boot, 36, 550)
    put32(boot, 44, 2)
    return boot


def gpt_sectors():
    table = bytearray(512)
    table[:16] = b'\x01' * 16
    put64(table, 32, 40)
    put64(table, 40, 80)
    header = bytearray(512)
    header[:8] = b'EFI PART'
    put32(header, 8, 0x10000)
    put32(header, 12, 92)
    put64(header, 24, 1)
    put64(header, 32, 99)
    put64(header, 40, 3)
    put64(header, 48, 98)
    put64(header, 72, 2)
    put32(header, 80, 4)
    put32(header, 84, 128)
    put32(header, 88, zlib.crc32(table))
    put32(header, 16, zlib.crc32(header[:92]))
    return {0: mbr([(0xee, 1, 99)]), 1: bytes(header), 2: bytes(table)}


class ChunkConnection:
    def __init__(self, mutate=None):
        self.commands = []
        self.mutate = mutate

    def terminal_command(self, command):
        self.commands.append(command)
        _, _, sector, offset, length = command.split()
        data = bytes(range(256))
        values = dict(sector=sector, offset=offset, length=length,
                      data=data.hex(), crc32=f'{zlib.crc32(data):08x}', read_us='12')
        if self.mutate:
            self.mutate(values)
        return dict(status='OK', data=' '.join(f'{k}={v}' for k, v in values.items()))


class MmcTests(unittest.TestCase):
    def media(self, data, sectors=100, cap=128):
        self.reads = []
        def read(sector):
            self.reads.append(sector)
            return bytes(data.get(sector, bytes(512)))
        return mmc.Media(sectors, read, cap)

    def test_sector_requires_both_ranges_and_independent_crc(self):
        connection = ChunkConnection()
        self.assertEqual(mmc.read_sector(connection, 7), bytes(range(256)) * 2)
        self.assertEqual(connection.commands, ['MMC READ 7 0 256', 'MMC READ 7 256 256'])
        for key, value in [('sector', '8'), ('offset', '128'), ('length', '255'),
                           ('crc32', '00000000'), ('data', '00')]:
            connection = ChunkConnection(lambda row: row.update({key: value}))
            with self.subTest(key=key), self.assertRaises(ValueError):
                mmc.read_sector(connection, 7)
            self.assertEqual(len(connection.commands), 1)

    def test_media_cache_and_absolute_cap(self):
        media = self.media({}, cap=2)
        media.read(0)
        media.read(0)
        media.read(1)
        with self.assertRaises(mmc.LimitReached):
            media.read(2)
        with self.assertRaises(ValueError):
            media.read(100)
        self.assertEqual(self.reads, [0, 1])

    def test_mbr_rejects_overflow_and_overlap_before_partition_reads(self):
        for entries in [[(0x0b, 90, 20)], [(0x0b, 2**32 - 1, 2**32 - 1)],
                        [(0x0b, 1, 50), (0x0b, 40, 30)], [(0x0b, 0, 20)]]:
            with self.subTest(entries=entries), self.assertRaises(ValueError):
                mmc.partitions(self.media({0: mbr(entries)}))
            self.assertEqual(self.reads, [0])

    def test_gpt_validates_table_crc_and_usable_bounds(self):
        data = gpt_sectors()
        scheme, partitions = mmc.partitions(self.media(data))
        self.assertEqual((scheme, partitions[0]['start'], partitions[0]['sectors']), ('gpt', 40, 41))
        bad = bytearray(data[2])
        bad[32] ^= 1
        with self.assertRaisesRegex(ValueError, 'table CRC'):
            mmc.partitions(self.media({**data, 2: bad}))
        header = bytearray(data[1])
        put64(header, 72, 2**64 - 1)
        put32(header, 16, 0)
        put32(header, 16, zlib.crc32(header[:92]))
        with self.assertRaises(ValueError):
            mmc.partitions(self.media({**data, 1: header}))

    def test_gpt_huge_entry_table_stops_before_reading_it(self):
        data = gpt_sectors()
        header = bytearray(data[1])
        put64(header, 32, 999999)
        put64(header, 40, 200000)
        put64(header, 48, 999998)
        put32(header, 80, 100000)
        put32(header, 16, 0)
        put32(header, 16, zlib.crc32(header[:92]))
        media = self.media({0: mbr([(0xee, 1, 999999)]), 1: header}, sectors=1000000)
        with self.assertRaises(mmc.LimitReached):
            mmc.partitions(media)
        self.assertEqual(self.reads, [0, 1])

    def test_superfloppy_root_sample_stays_private_structured_data(self):
        boot = fat32()
        root = bytearray(512)
        root[:11] = b'UPDATE  BIN'
        root[11] = 0x20
        put16(root, 26, 5)
        put32(root, 28, 1024)
        result = mmc.investigate(self.media({0: boot, 582: root}, sectors=70000))
        self.assertEqual(result['scheme'], 'superfloppy')
        fs = result['filesystems'][0]
        self.assertEqual(fs['kind'], 'fat32')
        self.assertEqual(fs['root']['entries'][0]['short_name'], 'UPDATE.BIN')
        self.assertTrue(fs['root']['complete'])
        self.assertEqual(self.reads, [0, 582])

    def test_cyclic_and_invalid_fat_root_chain_are_not_followed(self):
        for next_cluster in [2, 0, 0x0ffffff7, 69999]:
            fat = bytearray(512)
            put32(fat, 8, next_cluster)
            deleted = bytes([0xe5]) * 512
            media = self.media({0: fat32(), 582: deleted, 32: fat}, sectors=70000)
            geometry = mmc.fat_geometry(media, dict(start=0, sectors=70000))
            with self.subTest(next_cluster=next_cluster), self.assertRaises(ValueError):
                mmc.root_directory(media, geometry)
            self.assertEqual(self.reads, [0, 582, 32])

    def test_fat_geometry_rejects_small_table_and_root_outside_data(self):
        for offset, value in [(36, 1), (44, 70000)]:
            boot = fat32()
            put32(boot, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                mmc.fat_geometry(self.media({0: boot}, sectors=70000), dict(start=0, sectors=70000))

    def test_root_cap_is_explicitly_incomplete(self):
        media = self.media({582: bytes([0xe5]) * 512}, sectors=70000, cap=1)
        geometry = dict(bits=32, root_cluster=2, clusters=69000, sectors_per_cluster=1,
                        data_start=582, fat_start=32)
        self.assertEqual(mmc.root_directory(media, geometry), dict(entries=[], complete=False))
        self.assertEqual(self.reads, [582])

    def test_long_name_requires_matching_short_name_checksum(self):
        short = bytearray(32)
        short[:11] = b'RESOUR~1BIN'
        short[11] = 0x20
        checksum = 0
        for byte in short[:11]:
            checksum = (((checksum & 1) << 7) + (checksum >> 1) + byte) & 255
        lfn = bytearray(32)
        lfn[0], lfn[11], lfn[13] = 0x41, 0x0f, checksum
        raw = 'resource.bin'.encode('utf-16-le') + b'\0\0'
        lfn[1:11], lfn[14:26], lfn[28:32] = raw[:10], raw[10:22], raw[22:26]
        geometry = dict(bits=16, root_sectors=1, root_start=1)
        for correct in [True, False]:
            row = bytearray(lfn)
            if not correct:
                row[13] ^= 1
            root = row + short + bytes(448)
            result = mmc.root_directory(self.media({1: root}), geometry)
            self.assertEqual(result['entries'][0].get('long_name'), 'resource.bin' if correct else None)

    def test_unknown_and_extended_layout_are_not_guessed(self):
        self.assertEqual(mmc.investigate(self.media({}))['scheme'], 'unknown')
        result = mmc.investigate(self.media({0: mbr([(5, 1, 50)])}))
        self.assertEqual(result['filesystems'][0]['kind'], 'extended-unexamined')
        self.assertEqual(self.reads, [0])


if __name__ == '__main__':
    unittest.main()
