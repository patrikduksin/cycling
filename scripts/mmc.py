"""Read a bounded eMMC layout and root-directory sample into ignored private files."""
import argparse
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import zlib

from usb import UsbConnection

ROOT = Path(__file__).resolve().parents[1]
SECTOR_SIZE = 512
MAX_SECTORS = 128


class LimitReached(ValueError):
    pass


def u16(data, offset):
    return struct.unpack_from('<H', data, offset)[0]


def u32(data, offset):
    return struct.unpack_from('<I', data, offset)[0]


def u64(data, offset):
    return struct.unpack_from('<Q', data, offset)[0]


def checked_range(start, length, total):
    if type(start) is not int or type(length) is not int or start < 0 or length <= 0 or start >= total or length > total - start:
        raise ValueError('media range exceeds reported capacity')


def fields(reply):
    if reply.get('status') != 'OK':
        raise ValueError('device rejected read-only MMC operation')
    text = reply.get('data')
    if not isinstance(text, str):
        raise ValueError('invalid MMC reply')
    pairs = re.findall(r'(\w+)=([^\s]+)', text)
    result = dict(pairs)
    if len(result) != len(pairs):
        raise ValueError('duplicate MMC reply field')
    return result


def read_sector(connection, sector):
    """Read exactly one sector through two independently checked 256-byte replies."""
    if type(sector) is not int or not 0 <= sector < 2**64:
        raise ValueError('invalid sector number')
    output = bytearray()
    for offset in (0, 256):
        reply = fields(connection.terminal_command(f'MMC READ {sector} {offset} 256'))
        for name, expected in [('sector', sector), ('offset', offset), ('length', 256)]:
            if reply.get(name) != str(expected):
                raise ValueError('MMC read reply range mismatch')
        if not re.fullmatch(r'[0-9a-fA-F]{512}', reply.get('data', '')) or not re.fullmatch(r'[0-9a-fA-F]{8}', reply.get('crc32', '')):
            raise ValueError('invalid MMC read encoding')
        data = bytes.fromhex(reply['data'])
        if zlib.crc32(data) != int(reply['crc32'], 16):
            raise ValueError('MMC chunk CRC mismatch')
        output.extend(data)
    return bytes(output)


class Media:
    """One cached reader with a hard cap across partition and filesystem probes."""
    def __init__(self, sectors, reader, max_sectors=MAX_SECTORS, save=None):
        if type(sectors) is not int or sectors <= 0 or sectors > 2**64 - 1:
            raise ValueError('invalid media capacity')
        if not 1 <= max_sectors <= MAX_SECTORS:
            raise ValueError('sector cap must be between 1 and 128')
        self.sectors, self.reader, self.max_sectors, self.save = sectors, reader, max_sectors, save
        self.cache = {}

    def read(self, sector):
        checked_range(sector, 1, self.sectors)
        if sector not in self.cache:
            if len(self.cache) >= self.max_sectors:
                raise LimitReached('sector read cap reached')
            data = self.reader(sector)
            if len(data) != SECTOR_SIZE:
                raise ValueError('short media sector')
            self.cache[sector] = bytes(data)
            if self.save:
                self.save(sector, bytes(data))
        return self.cache[sector]


def nonoverlapping(partitions):
    ordered = sorted(partitions, key=lambda p: p['start'])
    for left, right in zip(ordered, ordered[1:]):
        if left['start'] + left['sectors'] > right['start']:
            raise ValueError('overlapping partitions')
    return partitions


def gpt(media):
    header = media.read(1)
    if header[:8] != b'EFI PART' or not 92 <= u32(header, 12) <= 512:
        raise ValueError('invalid GPT header')
    size = u32(header, 12)
    checked = bytearray(header[:size])
    checked[16:20] = b'\0' * 4
    if zlib.crc32(checked) != u32(header, 16) or u64(header, 24) != 1:
        raise ValueError('GPT header CRC or location mismatch')
    backup, first, last = u64(header, 32), u64(header, 40), u64(header, 48)
    checked_range(backup, 1, media.sectors)
    if first < 2 or last < first or last >= media.sectors or first <= backup <= last:
        raise ValueError('invalid GPT usable range')
    start, count, entry_size = u64(header, 72), u32(header, 80), u32(header, 84)
    if not count or entry_size < 128 or entry_size % 128 or entry_size > 4096:
        raise ValueError('invalid GPT entry geometry')
    length = count * entry_size
    table_sectors = (length + 511) // 512
    checked_range(start, table_sectors, media.sectors)
    if start < 2 or start + table_sectors > first:
        raise ValueError('GPT table overlaps usable media')
    if table_sectors > MAX_SECTORS:
        raise LimitReached('GPT entry table exceeds read cap')
    table = b''.join(media.read(start + i) for i in range(table_sectors))[:length]
    if zlib.crc32(table) != u32(header, 88):
        raise ValueError('GPT entry table CRC mismatch')
    partitions = []
    for i in range(count):
        entry = table[i * entry_size:(i + 1) * entry_size]
        if entry[:16] == b'\0' * 16:
            continue
        begin, end = u64(entry, 32), u64(entry, 40)
        if begin < first or end < begin or end > last:
            raise ValueError('GPT partition exceeds usable range')
        partitions.append(dict(index=i + 1, start=begin, sectors=end - begin + 1,
                               type_guid=entry[:16].hex(), name=entry[56:128].decode('utf-16-le', 'replace').rstrip('\0')))
    return nonoverlapping(partitions)


def partitions(media):
    boot = media.read(0)
    if boot[510:] != b'\x55\xaa':
        return 'unknown', []
    # A FAT boot jump and supported sector size distinguish superfloppy BPBs
    # before interpreting overlapping boot code as an MBR partition table.
    if boot[0] in (0xeb, 0xe9) and u16(boot, 11) == 512:
        return 'superfloppy', [dict(index=0, start=0, sectors=media.sectors)]
    result = []
    for index in range(4):
        row = boot[446 + index * 16:462 + index * 16]
        kind, start, size = row[4], u32(row, 8), u32(row, 12)
        if not kind and not start and not size:
            continue
        if row[0] not in (0, 0x80) or not kind or start == 0:
            raise ValueError('invalid MBR partition')
        checked_range(start, size, media.sectors)
        result.append(dict(index=index + 1, start=start, sectors=size, type=kind))
    if any(row['type'] == 0xee for row in result):
        if len(result) != 1:
            raise ValueError('hybrid MBR not supported')
        return 'gpt', gpt(media)
    return ('mbr' if result else 'unknown'), nonoverlapping(result)


def fat_geometry(media, partition):
    start, capacity = partition['start'], partition['sectors']
    boot = media.read(start)
    if boot[510:] != b'\x55\xaa' or boot[0] not in (0xeb, 0xe9):
        return None
    bps, spc, reserved, fats, roots = u16(boot, 11), boot[13], u16(boot, 14), boot[16], u16(boot, 17)
    total = u16(boot, 19) or u32(boot, 32)
    fat_size = u16(boot, 22) or u32(boot, 36)
    if bps != 512:
        return None
    if spc not in (1, 2, 4, 8, 16, 32, 64, 128) or not reserved or fats not in (1, 2) or not fat_size:
        raise ValueError('invalid FAT geometry')
    checked_range(0, total, capacity)
    root_sectors = (roots * 32 + 511) // 512
    metadata = reserved + fats * fat_size + root_sectors
    if metadata >= total:
        raise ValueError('FAT metadata exceeds volume')
    clusters = (total - metadata) // spc
    bits = 12 if clusters < 4085 else 16 if clusters < 65525 else 32
    required_fat_bytes = ((clusters + 2) * bits + 7) // 8
    if required_fat_bytes > fat_size * 512:
        raise ValueError('FAT too small for data clusters')
    active_fat = 0
    if bits == 32:
        if roots or u16(boot, 22) or u16(boot, 42):
            raise ValueError('invalid FAT32 fields')
        flags = u16(boot, 40)
        if flags & 0x80:
            active_fat = flags & 15
            if active_fat >= fats:
                raise ValueError('invalid active FAT')
        root_cluster = u32(boot, 44) & 0x0fffffff
        if not 2 <= root_cluster < clusters + 2:
            raise ValueError('invalid FAT32 root cluster')
    else:
        if not roots or not u16(boot, 22):
            raise ValueError('invalid FAT12/16 root fields')
        root_cluster = None
    return dict(kind=f'fat{bits}', bits=bits, start=start, sectors=total,
                sectors_per_cluster=spc, clusters=clusters,
                fat_start=start + reserved + active_fat * fat_size,
                fat_sectors=fat_size, data_start=start + metadata,
                root_start=start + reserved + fats * fat_size,
                root_sectors=root_sectors, root_cluster=root_cluster)


def directory_sectors(media, geometry, cap=32):
    if geometry['bits'] != 32:
        for i in range(geometry['root_sectors']):
            if i >= cap:
                raise LimitReached('root directory sample cap reached')
            yield media.read(geometry['root_start'] + i)
        return
    cluster, visited, used = geometry['root_cluster'], set(), 0
    while True:
        if cluster in visited:
            raise ValueError('cyclic FAT root chain')
        if not 2 <= cluster < geometry['clusters'] + 2:
            raise ValueError('invalid FAT root chain cluster')
        visited.add(cluster)
        base = geometry['data_start'] + (cluster - 2) * geometry['sectors_per_cluster']
        for offset in range(geometry['sectors_per_cluster']):
            if used >= cap:
                raise LimitReached('root directory sample cap reached')
            used += 1
            yield media.read(base + offset)
        fat_offset = cluster * 4
        entry = media.read(geometry['fat_start'] + fat_offset // 512)
        cluster = u32(entry, fat_offset % 512) & 0x0fffffff
        if cluster >= 0x0ffffff8:
            return
        if cluster == 0x0ffffff7 or cluster >= 0x0ffffff0:
            raise ValueError('bad or reserved FAT root chain cluster')


def root_directory(media, geometry):
    """Only immediate root entries; accept long names with a valid sequence/checksum."""
    entries = []
    fragments, expected, checksum = {}, 0, None
    complete = True
    try:
        for sector in directory_sectors(media, geometry):
            for offset in range(0, 512, 32):
                row = sector[offset:offset + 32]
                if row[0] == 0:
                    return dict(entries=entries, complete=True)
                if row[0] == 0xe5:
                    fragments, expected, checksum = {}, 0, None
                    continue
                if row[11] == 0x0f:
                    order = row[0] & 0x1f
                    if row[0] & 0x40:
                        fragments, expected, checksum = {}, order, row[13]
                    if (not 1 <= order <= 20 or row[0] & 0xa0 or order != expected
                            or checksum != row[13] or row[12] or u16(row, 26)):
                        fragments, expected, checksum = {}, 0, None
                    else:
                        fragments[order] = row[1:11] + row[14:26] + row[28:32]
                        expected -= 1
                    continue
                # Escape non-ASCII bytes instead of guessing the volume's OEM page.
                name = row[:8].decode('ascii', 'backslashreplace').rstrip()
                extension = row[8:11].decode('ascii', 'backslashreplace').rstrip()
                if extension:
                    name += '.' + extension
                cluster = u16(row, 26)
                if geometry['bits'] == 32:
                    cluster |= u16(row, 20) << 16
                entry = dict(short_name=name, short_name_hex=row[:11].hex(),
                             attributes=row[11], cluster=cluster, size=u32(row, 28))
                actual_checksum = 0
                for byte in row[:11]:
                    actual_checksum = (((actual_checksum & 1) << 7) + (actual_checksum >> 1) + byte) & 255
                if fragments and expected == 0 and checksum == actual_checksum:
                    raw = b''.join(fragments[i] for i in range(1, len(fragments) + 1))
                    units = [raw[i:i + 2] for i in range(0, len(raw), 2)]
                    if b'\0\0' in units:
                        end = units.index(b'\0\0')
                        valid_padding = all(unit == b'\xff\xff' for unit in units[end + 1:])
                        units = units[:end]
                    else:
                        valid_padding = True
                    if valid_padding and 1 <= len(units) <= 255 and b'\xff\xff' not in units:
                        try:
                            entry['long_name'] = b''.join(units).decode('utf-16-le')
                        except UnicodeDecodeError:
                            pass
                fragments, expected, checksum = {}, 0, None
                entries.append(entry)
    except LimitReached:
        complete = False
    return dict(entries=entries, complete=complete)


def investigate(media):
    scheme, rows = partitions(media)
    result = dict(scheme=scheme, sectors=media.sectors, partitions=rows, filesystems=[])
    for partition in rows:
        if partition.get('type') in (5, 15, 0x85):
            result['filesystems'].append(dict(partition=partition['index'], kind='extended-unexamined'))
            continue
        try:
            geometry = fat_geometry(media, partition)
            if geometry is None:
                result['filesystems'].append(dict(partition=partition['index'], kind='unknown'))
            else:
                directory = root_directory(media, geometry)
                result['filesystems'].append(dict(partition=partition['index'], **geometry, root=directory))
        except LimitReached:
            result['filesystems'].append(dict(partition=partition['index'], kind='read-cap-reached'))
        except ValueError as error:
            result['filesystems'].append(dict(partition=partition['index'], kind='invalid', error=str(error)))
    result['sectors_read'] = len(media.cache)
    return result


def private_output(value):
    path = Path(value).resolve()
    local = (ROOT / '.local').resolve()
    if local not in path.parents:
        raise ValueError('output must be inside repository .local')
    relative = path.relative_to(ROOT)
    if subprocess.run(['git', 'check-ignore', '-q', '--', str(relative)], cwd=ROOT).returncode:
        raise ValueError('output must be git-ignored')
    path.mkdir(parents=True, mode=0o700, exist_ok=False)
    path.chmod(0o700)
    return path


def save(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', help='new ignored directory under .local')
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--max-sectors', type=int, default=MAX_SECTORS)
    args = parser.parse_args()
    output = None
    try:
        if not 1 <= args.max_sectors <= MAX_SECTORS:
            raise ValueError('invalid read cap')
        output = private_output(args.output)
        sectors_dir = output / 'sectors'
        sectors_dir.mkdir(mode=0o700)
        save(output / 'usb.log', b'')
        with UsbConnection(args.port, output / 'usb.log') as connection:
            status = connection.terminal_command('MMC')
            save(output / 'status.json', json.dumps(status, indent=2).encode())
            capacity = fields(status).get('sectors', '')
            if not re.fullmatch(r'[0-9]+', capacity):
                raise ValueError('MMC status lacks sector capacity')
            media = Media(int(capacity), lambda sector: read_sector(connection, sector), args.max_sectors,
                          lambda sector, data: save(sectors_dir / f'{sector:016x}.bin', data))
            result = investigate(media)
        save(output / 'layout.json', json.dumps(result, indent=2).encode())
        kinds = ','.join(row['kind'] for row in result['filesystems']) or 'none'
        print(f"MMC read-only: layout={result['scheme']} partitions={len(result['partitions'])} filesystems={kinds} sectors_read={result['sectors_read']} cap={args.max_sectors}")
    except (ValueError, OSError, RuntimeError, TimeoutError, KeyError) as error:
        if output is not None:
            try:
                save(output / 'error.json', json.dumps(dict(error=type(error).__name__, detail=str(error))).encode())
            except OSError:
                pass
        raise SystemExit(f'MMC investigation failed ({type(error).__name__}); inspect private output.') from None


if __name__ == '__main__':
    main()
