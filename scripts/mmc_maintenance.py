"""Private, exclusive C606 MMC backup and explicitly armed maintenance transfers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import secrets
import struct
import sys
import time
import zlib

from usb import ROOT, acquire_lock, open_no_reset

HEADER = struct.Struct('<4s7I')
SECTOR = 512
TIMEOUT = 10


def crc(data):
    return zlib.crc32(data) & 0xffffffff


def private_path(value):
    path = Path(value).resolve()
    if not path.is_relative_to((ROOT / '.local').resolve()):
        raise ValueError('images and evidence must stay under ignored .local/')
    return path


def digest_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for data in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(data)
    return digest.hexdigest()


class Connection:
    def __init__(self, port):
        self.port = port
        self.fd = self.lock = None
        self.sequence = secrets.randbelow(0x7fffffff) + 1
        self.total = self.token = None

    def __enter__(self):
        try:
            self.lock = acquire_lock()
            self.fd = open_no_reset(self.port)
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_):
        try:
            if self.fd is not None:
                os.close(self.fd)
        finally:
            self.fd = None
            if self.lock is not None:
                self.lock.close()
                self.lock = None

    def send(self, data):
        deadline = time.monotonic() + TIMEOUT
        offset = 0
        while offset < len(data):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('USB write timeout; outcome uncertain, do not replay')
            if not select.select([], [self.fd], [], remaining)[1]:
                continue
            try:
                size = os.write(self.fd, data[offset:])
            except BlockingIOError:
                continue
            if not size:
                raise ConnectionError('USB disconnected; outcome uncertain')
            offset += size

    def receive(self, length, deadline=None):
        if deadline is None:
            deadline = time.monotonic() + TIMEOUT
        result = bytearray()
        while len(result) < length:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('USB response timeout; outcome uncertain, do not replay')
            if not select.select([self.fd], [], [], remaining)[0]:
                continue
            try:
                data = os.read(self.fd, length - len(result))
            except BlockingIOError:
                continue
            if not data:
                raise ConnectionError('USB disconnected; outcome uncertain')
            result.extend(data)
        return bytes(result)

    def request(self, op, start=0, count=0, token=0, payload=b''):
        self.sequence += 1
        head = HEADER.pack(b'C6RQ', op, self.sequence, start, count, token, crc(payload), 0)
        self.send(head[:28] + struct.pack('<I', crc(head[:28])) + payload)
        return self.sequence

    def response(self, request_id, initial_info=False):
        deadline = time.monotonic() + TIMEOUT
        header = self.receive(32, deadline)
        if initial_info:
            # Startup text and old frames may remain in the tty. Scan only for
            # this new INFO response; never resynchronize after a mutation.
            for discarded in range(4097):
                fields = HEADER.unpack(header)
                if fields[0] == b'C6RP' and fields[1] == request_id and crc(header[:28]) == fields[7]:
                    break
                if discarded == 4096:
                    raise ValueError('initial INFO synchronization exceeded 4096 bytes')
                header = header[1:] + self.receive(1, deadline)
        magic, ident, status, start, count, encoding, checksum, check = HEADER.unpack(header)
        if magic != b'C6RP' or crc(header[:28]) != check or ident != request_id:
            raise ValueError('response header CRC, magic or request ID mismatch')
        if status:
            if count != 0 or encoding != 3:
                raise ValueError('malformed failure response')
            raise RuntimeError(f'device operation failed with status {status}; no replay')
        if encoding in (0, 1):
            if not 1 <= count <= 7:
                raise ValueError('response exceeds bounded frame size')
            data = self.receive(count * SECTOR) if encoding == 0 else self.receive(1) * (count * SECTOR)
        elif encoding == 2:
            data = self.receive(16)
        elif encoding == 3:
            data = b''
        else:
            raise ValueError('unknown response encoding')
        if encoding != 3 and crc(data) != checksum:
            raise ValueError('response data CRC mismatch')
        return start, count, encoding, checksum, data

    def info(self):
        start, count, encoding, _, data = self.response(self.request(1), initial_info=True)
        if encoding != 2 or start != 0 or count != 0:
            raise ValueError('invalid INFO response')
        total, sector_size, token = struct.unpack('<QII', data)
        if not 0 < total <= 0xffffffff or sector_size != SECTOR or not token:
            raise ValueError('unsupported INFO geometry or session token')
        self.total, self.token = total, token
        return {'total_sectors': total, 'sector_size': sector_size, 'bytes': total * SECTOR}

    def bounds(self, start, count):
        if self.total is None or start < 0 or count <= 0 or start + count > self.total:
            raise ValueError('range outside identified medium')

    def read(self, start, count):
        self.bounds(start, count)
        ident = self.request(2, start, count)
        end = start + count
        while start < end:
            actual, size, encoding, _, data = self.response(ident)
            if encoding not in (0, 1) or actual != start or actual + size > end:
                raise ValueError('READ response is not the exact contiguous requested range')
            yield data
            start += size

    def ack(self, ident, start, count, checksum=None):
        actual, size, encoding, check, _ = self.response(ident)
        if (actual, size, encoding) != (start, count, 3) or (checksum is not None and checksum != check):
            raise ValueError('acknowledgment range or verified CRC mismatch; outcome uncertain')

    def arm(self, start, count):
        self.bounds(start, count)
        self.ack(self.request(3, start, count, self.token), start, count)

    def write_sector(self, start, data):
        self.bounds(start, 1)
        if len(data) != SECTOR:
            raise ValueError('WRITE requires exactly one sector')
        self.ack(self.request(4, start, 1, self.token, data), start, 1, crc(data))

    def recover(self):
        self.ack(self.request(5), 0, 0)

    def wide(self):
        if self.token is None or self.total is None:
            raise ValueError('WIDE requires a successful INFO in this session')
        self.ack(self.request(6), 0, 0)


class Progress:
    def __init__(self, label, total):
        self.label, self.total, self.done, self.last = label, total, 0, time.monotonic()

    def add(self, size):
        self.done += size
        now = time.monotonic()
        if now - self.last >= 5 or self.done == self.total:
            print(f'{self.label}: {self.done}/{self.total} bytes', file=sys.stderr, flush=True)
            self.last = now


def save_manifest(path, manifest):
    temporary = path.with_name(path.name + '.tmp')
    with temporary.open('x') as output:
        json.dump(manifest, output, indent=2)
        output.write('\n')
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def capture(connection, path, start, count, resume=False):
    connection.bounds(start, count)
    digest = hashlib.sha256()
    progress = Progress('read', count * SECTOR)
    completed = path.stat().st_size if resume else 0
    if completed % SECTOR or completed > count * SECTOR:
        raise ValueError('resumed image must contain a whole-sector prefix within medium capacity')
    with path.open('r+b' if resume else 'x+b') as output:
        for data in iter(lambda: output.read(1024 * 1024), b''):
            digest.update(data)
        if output.tell() != completed:
            raise ValueError('backup prefix changed while opening resume file')
        output.seek(completed)
        progress.add(completed)
        try:
            remaining = count - completed // SECTOR
            if remaining:
                for data in connection.read(start + completed // SECTOR, remaining):
                    if not data or len(data) % SECTOR or completed + len(data) > count * SECTOR:
                        raise ValueError('capture received an invalid complete-sector frame')
                    if not any(data):
                        output.seek(len(data), os.SEEK_CUR)
                    elif output.write(data) != len(data):
                        raise OSError('short backup file write')
                    digest.update(data)
                    completed += len(data)
                    progress.add(len(data))
            if completed != count * SECTOR:
                raise ValueError('capture ended before the requested range was complete')
        finally:
            # A sparse zero frame advances position without extending file size.
            # Preserve exactly the fully validated frames even when USB fails.
            output.truncate(completed)
            output.flush()
            os.fsync(output.fileno())
    return digest.hexdigest()


def verify(connection, path):
    if path.stat().st_size != connection.total * SECTOR:
        raise ValueError('verification requires an exact whole-medium image')
    progress = Progress('verify', path.stat().st_size)
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for data in connection.read(0, connection.total):
            if source.read(len(data)) != data:
                raise ValueError('device differs from backup; verification failed')
            digest.update(data)
            progress.add(len(data))
        if source.read(1):
            raise ValueError('image grew during verification')
    return digest.hexdigest()


def backup(connection, path, resume=False):
    manifest_path = Path(str(path) + '.json')
    if resume:
        manifest = json.loads(manifest_path.read_text())
        if (manifest.get('version') != 1 or manifest.get('verified') is not False
                or type(manifest.get('complete')) is not bool
                or manifest.get('total_sectors') != connection.total
                or manifest.get('sector_size') != SECTOR
                or private_path(manifest['image']) != path.resolve()):
            raise ValueError('resume requires an unverified manifest with this exact image and geometry')
        size = path.stat().st_size
        if size % SECTOR or size > connection.total * SECTOR:
            raise ValueError('resume image is not a whole-sector prefix within medium capacity')
        if manifest['complete']:
            if size != connection.total * SECTOR or digest_file(path) != manifest.get('sha256'):
                raise ValueError('completed backup image differs from its recorded size or SHA256')
            digest = manifest['sha256']
        else:
            digest = capture(connection, path, 0, connection.total, resume=True)
    else:
        if path.exists() or manifest_path.exists():
            raise ValueError('backup image and manifest must be new; use explicit --resume for an unverified backup')
        manifest = {'version': 1, 'image': str(path), 'total_sectors': connection.total,
                    'sector_size': SECTOR, 'complete': False, 'verified': False}
        save_manifest(manifest_path, manifest)
        digest = capture(connection, path, 0, connection.total)
    if not manifest['complete']:
        manifest.update(complete=True, sha256=digest)
        save_manifest(manifest_path, manifest)
    if verify(connection, path) != digest:
        raise ValueError('backup digest changed between independent reads')
    manifest.update(verified=True, verified_at=time.time())
    save_manifest(manifest_path, manifest)
    return manifest


def validate_backup(path, total, single_read_copy=None):
    manifest = json.loads(path.read_text())
    if (manifest.get('version') != 1 or manifest.get('complete') is not True
            or manifest.get('total_sectors') != total
            or manifest.get('sector_size') != SECTOR):
        raise ValueError('requires a completed independently verified whole-medium backup')
    if manifest.get('verified') is not True and (manifest.get('verified') is not False or single_read_copy is None):
        raise ValueError('requires independent full verification or an explicit single-read copy exception')
    image = private_path(manifest['image'])
    if image.stat().st_size != total * SECTOR or digest_file(image) != manifest.get('sha256'):
        raise ValueError('backup image no longer matches verified manifest')
    if manifest['verified']:
        return 'independent_full_media_read'
    # Explicit owner-authorized exception. A matching separate copy protects
    # capture durability; it does not independently verify the source medium.
    copy = Path(single_read_copy).resolve()
    if not copy.is_file() or os.path.samefile(image, copy):
        raise ValueError('single-read copy must be a separate regular file with a different inode')
    if copy.stat().st_size != total * SECTOR or digest_file(copy) != manifest['sha256']:
        raise ValueError('single-read copy size or SHA256 does not match the complete backup')
    return 'single_media_read_with_matching_copy'


def write_image(connection, image, start, expected_sha256, manifest, single_read_copy=None):
    validate_backup(manifest, connection.total, single_read_copy=single_read_copy)
    size = image.stat().st_size
    if size == 0 or size % SECTOR:
        raise ValueError('write image must contain whole sectors')
    connection.bounds(start, size // SECTOR)
    # Use one open descriptor and check the content again as transmitted.
    with image.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
        if digest != expected_sha256.lower():
            raise ValueError('write image SHA256 does not match explicit confirmation')
        source.seek(0)
        connection.arm(start, size // SECTOR)
        progress = Progress('write verified', size)
        sent = hashlib.sha256()
        for index in range(size // SECTOR):
            data = source.read(SECTOR)
            sent.update(data)
            connection.write_sector(start + index, data)
            progress.add(len(data))
        if source.read(1) or sent.hexdigest() != digest:
            raise ValueError('write source changed during transfer; inspect device, do not retry')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('info')
    commands.add_parser('recover')
    commands.add_parser('wide', help='explicitly switch to four-bit reads and disarm writes')
    backup_parser = commands.add_parser('backup')
    backup_parser.add_argument('image', type=private_path)
    backup_parser.add_argument('--resume', action='store_true',
                               help='explicitly continue an existing unverified backup after inspection')
    commands.add_parser('verify').add_argument('image', type=private_path)
    read = commands.add_parser('read')
    read.add_argument('image', type=private_path)
    read.add_argument('--start', type=int, required=True)
    read.add_argument('--count', type=int, required=True)
    write = commands.add_parser('write')
    write.add_argument('image', type=private_path)
    write.add_argument('--start', type=int, required=True)
    write.add_argument('--confirm-sha256', required=True)
    write.add_argument('--backup-manifest', type=private_path, required=True)
    write.add_argument('--single-read-copy', type=Path,
                       help='explicitly restore using one complete capture and a separate matching copy')
    args = parser.parse_args()
    with Connection(args.port) as connection:
        if args.command == 'recover':
            connection.recover()
            print('MMC recovery acknowledged.')
            return
        info = connection.info()
        if args.command == 'info':
            print(json.dumps(info))
        elif args.command == 'wide':
            connection.wide()
            print('Four-bit read mode acknowledged; writes disarmed.')
        elif args.command == 'backup':
            backup(connection, args.image, resume=args.resume)
            print('Full backup independently verified; private manifest saved.')
        elif args.command == 'verify':
            verify(connection, args.image)
            print('Whole-medium verification passed.')
        elif args.command == 'read':
            capture(connection, args.image, args.start, args.count)
        elif args.command == 'write':
            write_image(connection, args.image, args.start, args.confirm_sha256, args.backup_manifest,
                        single_read_copy=args.single_read_copy)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(f'Stopped: {error}', file=sys.stderr)
        sys.exit(1)
