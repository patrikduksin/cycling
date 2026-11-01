"""Inspect the C606 MMC volumes, write and verify the TERRAIN pack, manage FILES and STORE.

Bulk data moves through the firmware's binary transfer session: `VOLUME TRANSFER`
on the ordinary terminal, then CRC-checked frames until `END` (layout in
`packages/console/src/transfer.rs`). The device pauses the app meanwhile and
refuses while a ride records. Writes reach only the TERRAIN pack's data sectors
in order, its header commit, the FILES store and deletes in STORE; RIDES is
never touched.

A terrain write is resumable: a pack left `writing` with the same length, CRC and
SHA-256 is read back chunk by chunk against the file and continues from the first
mismatch. It becomes `ready` only after every sector was written or verified and
the device's running CRC matches.
"""

import argparse
import hashlib
import json
import os
import select
import struct
import sys
import time
import zlib
from pathlib import Path

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT, private_path

FRAME = struct.Struct('<4sHHIQIIII')
SECTOR = 512
MAX_PAYLOAD = 4096
VERIFY_SECTORS = 256
TIMEOUT = 15

OP = {
    'hello': 1,
    'end': 2,
    'volumes': 3,
    'terrain_status': 10,
    'terrain_begin': 11,
    'terrain_resume': 12,
    'terrain_write': 13,
    'terrain_verify': 14,
    'terrain_commit': 15,
    'terrain_crc': 16,
    'files_status': 20,
    'files_format': 21,
    'files_list': 22,
    'files_begin': 23,
    'files_write': 24,
    'files_commit': 25,
    'files_read': 26,
    'files_delete': 27,
    'files_stat': 28,
    'store_status': 30,
    'store_list': 31,
    'store_read': 32,
    'store_delete': 33,
}
STATUS = [
    'ok',
    'invalid',
    'media',
    'state',
    'range',
    'mismatch',
    'stale',
    'full',
    'not_found',
    'foreign',
    'uninitialized',
    'too_large',
    'corrupt',
    'empty',
    'hole',
]
LAYOUTS = {0: 'blank', 1: 'legacy', 2: 'volumes', 3: 'foreign', 4: 'lost'}
PACK_STATES = {0: 'empty', 1: 'writing', 2: 'ready'}
NAME = 40
ENTRY = 2 + NAME + 12
STORE_ENTRY = NAME + 8
# Records one STORE_READ asks for; the device sends as many as fit 4 KiB.
STORE_RECORDS = 16


def crc(data, value=0):
    return zlib.crc32(data, value) & 0xFFFFFFFF


class TransferError(RuntimeError):
    def __init__(self, op, status):
        self.status = STATUS[status] if status < len(STATUS) else str(status)
        super().__init__(f'{op} failed: {self.status}')


class Transfer:
    """One binary session over an open no-reset terminal descriptor."""

    def __init__(self, fd, log=None):
        self.fd, self.log, self.id = fd, log, 0
        self.pending = bytearray()

    def _send(self, data, deadline):
        offset = 0
        while offset < len(data):
            if time.monotonic() >= deadline:
                raise TimeoutError('USB write timeout; the request outcome is uncertain')
            if not select.select([], [self.fd], [], 0.2)[1]:
                continue
            try:
                offset += os.write(self.fd, data[offset:])
            except BlockingIOError:
                continue

    def _fill(self, size, deadline):
        while len(self.pending) < size:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('transfer reply timed out; the request outcome is uncertain')
            if not select.select([self.fd], [], [], min(0.2, remaining))[0]:
                continue
            try:
                data = os.read(self.fd, 65536)
            except BlockingIOError:
                continue
            if not data:
                raise RuntimeError('device disconnected; the request outcome is uncertain')
            if self.log:
                self.log.write(data)
            self.pending += data

    def call(self, name, arg=0, count=0, payload=b'', allow=()):
        """Send one request; return (status, arg, count, payload). Raises unless ok or allowed."""
        self.id += 1
        head = FRAME.pack(b'C6VQ', OP[name], 0, self.id, arg, count, len(payload), crc(payload), 0)
        head = head[:32] + struct.pack('<I', crc(head[:32]))
        deadline = time.monotonic() + TIMEOUT
        self._send(head + payload, deadline)
        while True:
            self._fill(4, deadline)
            start = self.pending.find(b'C6VP')
            if start < 0:
                # Keep a possible partial magic and wait for more bytes.
                del self.pending[: len(self.pending) - 3]
                self._fill(len(self.pending) + 1, deadline)
                continue
            del self.pending[:start]
            self._fill(FRAME.size, deadline)
            fields = FRAME.unpack(bytes(self.pending[: FRAME.size]))
            if fields[8] != crc(bytes(self.pending[:32])):
                del self.pending[:1]
                continue
            _, status, op, reply_id, reply_arg, reply_count, length, data_crc, _ = fields
            self._fill(FRAME.size + length, deadline)
            data = bytes(self.pending[FRAME.size : FRAME.size + length])
            del self.pending[: FRAME.size + length]
            if reply_id != self.id or op != OP[name]:
                continue
            if crc(data) != data_crc:
                raise RuntimeError(f'{name} reply payload corrupted in transit')
            if status != 0 and STATUS[status] not in allow:
                raise TransferError(name, status)
            return STATUS[status] if status < len(STATUS) else str(status), reply_arg, reply_count, data


def open_session(connection):
    reply = connection.terminal_command('VOLUME TRANSFER')
    if reply['status'] != 'ACCEPTED':
        raise RuntimeError(f'VOLUME TRANSFER refused: {reply["status"]} {reply["data"]}')
    transfer = Transfer(connection.fd, connection.log)
    hello = transfer.call('hello')[3]
    sectors, max_payload, version = struct.unpack('<QII', hello[:16])
    if version != 1 or max_payload < MAX_PAYLOAD:
        raise RuntimeError(f'unsupported transfer session version={version} max_payload={max_payload}')
    transfer.region = sectors
    return transfer


def decode_volumes(data):
    layout = LAYOUTS.get(data[0], str(data[0]))
    region, generation, planned, unallocated = struct.unpack('<QQQQ', data[8:40])
    volumes = []
    for offset in range(40, len(data), 25):
        entry = data[offset : offset + 25]
        name = entry[:8].rstrip(b'\0').decode()
        start, sectors = struct.unpack('<QQ', entry[9:25])
        volumes.append({'name': name, 'start': start, 'sectors': sectors})
    result = {'layout': layout, 'sectors': region, 'volumes': volumes}
    if layout == 'volumes':
        result.update(generation=generation, planned=planned, unallocated=unallocated)
    return result


def decode_terrain(data):
    state = PACK_STATES.get(data[0], str(data[0]))
    generation, length, checksum = struct.unpack('<QQI', data[1:21])
    stamp, capacity, cursor = struct.unpack('<QQQ', data[69:93])
    result = {'state': state, 'capacity': capacity}
    if state != 'empty':
        result.update(
            generation=generation,
            length=length,
            crc32=f'{checksum:08x}',
            tag=data[21:37].rstrip(b'\0').decode(errors='replace'),
            sha256=data[37:69].hex(),
            stamp=stamp,
            cursor=cursor,
        )
    return result


def decode_entry(data):
    slot = struct.unpack('<H', data[:2])[0]
    name = data[2 : 2 + NAME].rstrip(b'\0').decode()
    length, checksum, revision = struct.unpack('<III', data[2 + NAME : ENTRY])
    return {'slot': slot, 'name': name, 'length': length, 'crc32': f'{checksum:08x}', 'revision': revision}


def inspect(transfer):
    result = decode_volumes(transfer.call('volumes')[3])
    if result['layout'] != 'volumes':
        return result
    status, _, _, data = transfer.call('terrain_status', allow=('foreign',))
    result['terrain'] = decode_terrain(data) if status == 'ok' else {'state': status}
    status, _, _, data = transfer.call('files_status', allow=('uninitialized', 'foreign', 'corrupt'))
    if status == 'ok':
        generation, store, files, capacity, max_file, data_sectors = struct.unpack('<QQIIIQ', data[:36])
        result['files'] = {
            'state': 'ready',
            'files': files,
            'capacity': capacity,
            'max_file': max_file,
            'generation': generation,
            'store': f'{store:016x}',
            'data_sectors': data_sectors,
        }
    else:
        result['files'] = {'state': status}
    status, _, _, data = transfer.call('store_status', allow=('uninitialized', 'foreign', 'corrupt', 'invalid'))
    if status == 'ok':
        result['store'] = decode_store_status(data)
    else:
        # Firmware from before STORE does not know the request.
        result['store'] = {'state': 'unsupported' if status == 'invalid' else status}
    return result


def decode_store_status(data):
    generation, store = struct.unpack('<QQ', data[:16])
    files, capacity, chunks, free, chunk_sectors, record = struct.unpack('<6I', data[16:40])
    return {
        'state': 'ready',
        'files': files,
        'capacity': capacity,
        'chunks': chunks,
        'free_chunks': free,
        'chunk_bytes': chunk_sectors * record,
        'generation': generation,
        'store': f'{store:016x}',
    }


def digest_file(path):
    """CRC32, SHA-256 and length, streamed."""
    sha, value, length = hashlib.sha256(), 0, 0
    with Path(path).open('rb') as source:
        while chunk := source.read(1 << 20):
            sha.update(chunk)
            value = crc(chunk, value)
            length += len(chunk)
    return value, sha.digest(), length


def sectors_of(source, first, count):
    """`count` sectors of the pack from sector `first`, the last one zero padded."""
    source.seek(first * SECTOR)
    data = source.read(count * SECTOR)
    return data + bytes(count * SECTOR - len(data))


def progress(label, done, total, started):
    elapsed = max(time.monotonic() - started, 1e-3)
    rate = done / elapsed
    eta = (total - done) / rate if rate else 0
    print(
        f'\r{label} {done / 2**20:.1f}/{total / 2**20:.1f} MiB {rate / 1024:.0f} KiB/s eta {eta:.0f}s',
        end='',
        file=sys.stderr,
        flush=True,
    )


def tag_bytes(tag):
    raw = tag.encode()
    if not 0 < len(raw) <= 16 or not all(33 <= b < 127 for b in raw):
        raise ValueError('tag must be 1 to 16 printable ASCII characters')
    return raw.ljust(16, b'\0')


def terrain_write(transfer, path, tag, resume=True, verify=False, force=False, digest=None):
    """Install `path` as the TERRAIN pack; returns the final status.

    Pass `digest` (from `digest_file`) computed before the session opens: hashing a
    large pack inside it can outlast the device's idle timeout."""
    tag_bytes(tag)
    checksum, sha, length = digest or digest_file(path)
    total = -(-length // SECTOR)
    status = decode_terrain(transfer.call('terrain_status')[3])
    # The tag is part of the identity: the same bytes under another format tag are another pack.
    same = status['state'] != 'empty' and (
        status['length'] == length
        and status['crc32'] == f'{checksum:08x}'
        and status['sha256'] == sha.hex()
        and status['tag'] == tag
    )
    if same and status['state'] == 'ready' and not force:
        print('terrain pack already installed', file=sys.stderr)
        if verify:
            terrain_verify(transfer, path, (checksum, sha, length))
        return status
    if length > status['capacity']:
        raise ValueError(f'pack of {length} bytes exceeds the TERRAIN capacity of {status["capacity"]}')
    cursor = 0
    started = time.monotonic()
    with Path(path).open('rb') as source:
        if same and status['state'] == 'writing' and resume:
            transfer.call('terrain_resume')
            while cursor < total:
                count = min(VERIFY_SECTORS, total - cursor)
                expected = crc(sectors_of(source, cursor, count))
                _, cursor, advanced, _ = transfer.call('terrain_verify', cursor, count, struct.pack('<I', expected))
                if not advanced:
                    break
                progress('verified', cursor * SECTOR, length, started)
            print(f'\nresuming at sector {cursor} of {total}', file=sys.stderr)
        else:
            spec = struct.pack('<QI', length, checksum) + tag_bytes(tag) + sha + struct.pack('<Q', int(time.time()))
            transfer.call('terrain_begin', payload=spec)
        started = time.monotonic()
        step = MAX_PAYLOAD // SECTOR
        while cursor < total:
            count = min(step, total - cursor)
            _, cursor, _, _ = transfer.call('terrain_write', cursor, 0, sectors_of(source, cursor, count))
            if cursor % 2048 == 0 or cursor == total:
                progress('written', min(cursor * SECTOR, length), length, started)
        print(file=sys.stderr)
        result = decode_terrain(transfer.call('terrain_commit')[3])
        if verify:
            terrain_verify(transfer, path, (checksum, sha, length))
    return result


def terrain_verify(transfer, path, digest=None):
    """Read the ready pack back as device-side CRCs of every chunk and compare."""
    checksum, sha, length = digest or digest_file(path)
    status = decode_terrain(transfer.call('terrain_status')[3])
    if status['state'] != 'ready':
        raise RuntimeError(f'terrain pack is {status["state"]}')
    if (status['length'], status['crc32'], status['sha256']) != (length, f'{checksum:08x}', sha.hex()):
        raise RuntimeError('the installed terrain pack is a different file')
    total = -(-length // SECTOR)
    started = time.monotonic()
    with Path(path).open('rb') as source:
        for first in range(0, total, VERIFY_SECTORS):
            count = min(VERIFY_SECTORS, total - first)
            device = struct.unpack('<I', transfer.call('terrain_crc', first, count)[3])[0]
            if device != crc(sectors_of(source, first, count)):
                raise RuntimeError(f'terrain sectors {first}..{first + count} differ from the file')
            progress('verified', min((first + count) * SECTOR, length), length, started)
    print(file=sys.stderr)
    return status


def files_list(transfer):
    entries, slot = [], 0
    while slot < 128:
        _, slot, count, data = transfer.call('files_list', slot, 128)
        entries += [decode_entry(data[i : i + ENTRY]) for i in range(0, count * ENTRY, ENTRY)]
    return entries


def files_put(transfer, name, path):
    data_crc, _, length = digest_file(path)
    name_bytes = name.encode()
    transfer.call('files_begin', 0, length, name_bytes)
    with Path(path).open('rb') as source:
        written = 0
        while written < length:
            chunk = source.read(MAX_PAYLOAD)
            _, written, _, _ = transfer.call('files_write', written, 0, chunk)
    entry = decode_entry(transfer.call('files_commit', 0, data_crc)[3])
    # Read the committed file back and compare.
    stat = decode_entry(transfer.call('files_stat', payload=name_bytes)[3])
    if (stat['length'], stat['crc32']) != (length, f'{data_crc:08x}'):
        raise RuntimeError('committed file does not match')
    value, offset = 0, 0
    while offset < length:
        data = transfer.call('files_read', offset, MAX_PAYLOAD, name_bytes)[3]
        if not data:
            raise RuntimeError('short file readback')
        value = crc(data, value)
        offset += len(data)
    if value != data_crc:
        raise RuntimeError('file readback differs')
    return {**entry, 'name': name}


def files_get(transfer, name, output):
    name_bytes = name.encode()
    stat = decode_entry(transfer.call('files_stat', payload=name_bytes)[3])
    partial = Path(f'{output}.partial')
    value, offset = 0, 0
    with partial.open('wb') as out:
        while offset < stat['length']:
            data = transfer.call('files_read', offset, MAX_PAYLOAD, name_bytes)[3]
            if not data:
                raise RuntimeError('short file read')
            out.write(data)
            value = crc(data, value)
            offset += len(data)
    if f'{value:08x}' != stat['crc32']:
        partial.unlink()
        raise RuntimeError('file read does not match its CRC')
    partial.replace(output)
    return {**stat, 'name': name}


def store_list(transfer, directory=''):
    """Files in `directory` (a prefix ending in `/`, or empty for all) and below."""
    _, _, count, data = transfer.call('store_list', payload=directory.encode())
    entries = []
    for offset in range(0, count * STORE_ENTRY, STORE_ENTRY):
        entry = data[offset : offset + STORE_ENTRY]
        file_id, chunks = struct.unpack('<II', entry[NAME:])
        entries.append({'name': entry[:NAME].rstrip(b'\0').decode(), 'id': file_id, 'chunks': chunks})
    return sorted(entries, key=lambda entry: entry['id'])


def store_get(transfer, name, output):
    """Pull every record of `name` into `output`.

    Records the medium lost end the pull: `output` keeps the records before
    them and the result names the first lost record. Any other failure
    leaves no file.
    """
    name_bytes = name.encode()
    partial = Path(f'{output}.partial')
    record, records, length, hole = 0, 0, 0, None
    try:
        with partial.open('wb') as out:
            while True:
                status, record, count, data = transfer.call(
                    'store_read', record, STORE_RECORDS, name_bytes, allow=('hole',)
                )
                if status == 'hole':
                    hole = {'record': record, 'records': count}
                    break
                if count == 0:
                    break
                out.write(data)
                records += count
                length += len(data)
    except BaseException:
        partial.unlink(missing_ok=True)
        raise
    partial.replace(output)
    result = {'name': name, 'records': records, 'bytes': length}
    return {**result, 'hole': hole} if hole else result


# Table writes and the store format run as ordinary terminal commands,
# outside the transfer session, which keeps STORE open.
LINE_ACTIONS = {
    ('init', None): 'VOLUME INIT',
    ('add-store', None): 'VOLUME ADD STORE',
    ('store', 'format'): 'STORE FORMAT CONFIRM',
}


def run(args, port):
    command = LINE_ACTIONS.get((args.action, getattr(args, 'verb', None)))
    if command == 'STORE FORMAT CONFIRM' and not args.confirm:
        raise ValueError('store format erases every file; pass --confirm')
    log_dir = private_path(ROOT / '.local' / 'volumes')
    log_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    # Hash packs before the transfer session starts, so its idle timeout cannot expire meanwhile.
    digest = digest_file(args.file) if args.action in ('terrain-write', 'terrain-verify') else None
    with UsbConnection(port, log_dir / f'usb-{time.time_ns()}.log') as connection:
        if command:
            reply = connection.terminal_command(command)
            if reply['status'] != 'OK':
                raise RuntimeError(f'{command}: {reply["status"]} {reply["data"]}')
            return {'status': reply['data']}
        transfer = open_session(connection)
        try:
            if args.action == 'inspect':
                return inspect(transfer)
            if args.action == 'terrain-write':
                return terrain_write(transfer, args.file, args.tag, not args.restart, args.verify, args.force, digest)
            if args.action == 'terrain-verify':
                return terrain_verify(transfer, args.file, digest)
            if args.action == 'files':
                if args.verb == 'list':
                    return files_list(transfer)
                if args.verb == 'put':
                    return files_put(transfer, args.name, args.path)
                if args.verb == 'get':
                    return files_get(transfer, args.name, args.path)
                if args.verb == 'delete':
                    transfer.call('files_delete', payload=args.name.encode())
                    return {'deleted': args.name}
                if args.verb == 'format':
                    if not args.confirm:
                        raise ValueError('files format erases every file; pass --confirm')
                    transfer.call('files_format', int.from_bytes(os.urandom(8), 'little'))
                    return {'formatted': True}
            if args.action == 'store':
                if args.verb == 'list':
                    return store_list(transfer, args.directory)
                if args.verb == 'get':
                    return store_get(transfer, args.name, args.path)
                if args.verb == 'delete':
                    transfer.call('store_delete', payload=args.name.encode())
                    return {'deleted': args.name}
            raise AssertionError(args.action)
        finally:
            transfer.call('end')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    add_port_argument(parser)
    actions = parser.add_subparsers(dest='action', required=True)
    actions.add_parser('inspect', help='volume table, terrain pack and file store')
    actions.add_parser('init', help='write the default volume table over a blank or legacy region')
    actions.add_parser(
        'add-store',
        help='add the STORE volume to a table from before it, in the unallocated tail, and format its store',
    )
    write = actions.add_parser('terrain-write', help='install a terrain pack from a file (resumable)')
    write.add_argument('file', type=Path)
    write.add_argument('--tag', required=True, help='pack format tag, up to 16 ASCII characters')
    write.add_argument('--verify', action='store_true', help='read every sector back afterwards')
    write.add_argument('--restart', action='store_true', help='rewrite from the start instead of resuming')
    write.add_argument('--force', action='store_true', help='rewrite even if this pack is installed')
    verify = actions.add_parser('terrain-verify', help='compare the installed pack with a file')
    verify.add_argument('file', type=Path)
    files = actions.add_parser('files', help='list, put, get, delete or format FILES')
    verbs = files.add_subparsers(dest='verb', required=True)
    verbs.add_parser('list')
    put = verbs.add_parser('put')
    put.add_argument('name')
    put.add_argument('path', type=Path)
    get = verbs.add_parser('get')
    get.add_argument('name')
    get.add_argument('path', type=Path)
    delete = verbs.add_parser('delete')
    delete.add_argument('name')
    fmt = verbs.add_parser('format')
    fmt.add_argument('--confirm', action='store_true')
    store = actions.add_parser('store', help='list, get or delete files in STORE, or erase it')
    verbs = store.add_subparsers(dest='verb', required=True)
    listing = verbs.add_parser('list')
    listing.add_argument('directory', nargs='?', default='', help='a prefix ending in /, such as traces/')
    get = verbs.add_parser('get')
    get.add_argument('name')
    get.add_argument('path', type=Path)
    delete = verbs.add_parser('delete')
    delete.add_argument('name')
    fmt = verbs.add_parser('format', help='erase every file and start an empty store')
    fmt.add_argument('--confirm', action='store_true')
    args = parser.parse_args()
    for path in (getattr(args, 'file', None),):
        if path is not None and not path.is_file():
            parser.error(f'{path} is not a file')
    result = run(args, selected_port(parser, args.port))
    print(json.dumps(result, indent=2))
    if isinstance(result, dict) and 'hole' in result:
        hole = result['hole']
        raise SystemExit(
            f'{args.name}: the device lost {hole["records"]} record(s) from record {hole["record"]}; '
            f'{args.path} holds the {result["records"]} records before them'
        )


if __name__ == '__main__':
    main()
