import hashlib
import os
import struct
import tempfile
import unittest
import zlib
from argparse import Namespace
from pathlib import Path
from unittest import mock

from cycling_devtools.device import volumes
from cycling_devtools.device.volumes import FRAME, OP, SECTOR, Transfer, crc


def frame(magic, first, second, ident, arg=0, count=0, payload=b''):
    head = FRAME.pack(magic, first, second, ident, arg, count, len(payload), crc(payload), 0)
    return head[:32] + struct.pack('<I', crc(head[:32])) + payload


class FakeDevice:
    """The device side of the session, enough to exercise the host algorithms."""

    def __init__(self, capacity=1 << 20):
        self.capacity = capacity
        self.sectors = {}
        self.header = None  # (state, length, crc, tag, sha)
        self.cursor = None
        self.running = 0
        self.files = {}
        self.writer = None
        self.writes = []
        self.fail_write_at = None

    def terrain_info(self):
        info = bytearray(93)
        if self.header:
            state, length, checksum, tag, sha = self.header
            info[0] = state
            info[1:21] = struct.pack('<QQI', 1, length, checksum)
            info[21:37] = tag
            info[37:69] = sha
        info[77:93] = struct.pack('<QQ', self.capacity, self.cursor or 0)
        return bytes(info)

    def accept(self, index, data):
        length = self.header[1]
        used = min(SECTOR, length - index * SECTOR)
        self.running = zlib.crc32(data[:used], self.running)
        self.cursor += 1

    def handle(self, op, arg, count, payload):
        """Returns (status, arg, count, payload)."""
        if op == OP['hello']:
            return 0, 0, 0, struct.pack('<QII', 5_777_407, 4096, 1)
        if op == OP['end']:
            return 0, 0, 0, b''
        if op == OP['volumes']:
            table = bytearray(40)
            table[0] = 2
            table[8:40] = struct.pack('<QQQQ', 5_777_407, 1, 5_777_407, 532_479)
            for name, kind, start, sectors in [
                (b'RIDES', 1, 2048, 2_097_152),
                (b'FILES', 3, 2_099_200, 524_288),
                (b'TERRAIN', 2, 2_623_488, 2_621_440),
            ]:
                table += name.ljust(8, b'\0') + bytes([kind]) + struct.pack('<QQ', start, sectors)
            return 0, 0, 3, bytes(table)
        if op == OP['terrain_status']:
            return 0, 0, 0, self.terrain_info()
        if op == OP['terrain_begin']:
            length, checksum = struct.unpack('<QI', payload[:12])
            self.header = (1, length, checksum, payload[12:28], payload[28:60])
            self.cursor, self.running = 0, 0
            return 0, 0, 0, self.terrain_info()
        if op == OP['terrain_resume']:
            if not self.header or self.header[0] != 1:
                return 3, 0, 0, b''
            self.cursor, self.running = 0, 0
            return 0, 0, 0, self.terrain_info()
        if op == OP['terrain_write']:
            if arg != self.cursor:
                return 4, 0, 0, b''
            for offset in range(0, len(payload), SECTOR):
                if self.fail_write_at == self.cursor:
                    self.fail_write_at = None
                    return 2, self.cursor, 0, b''
                data = payload[offset : offset + SECTOR]
                self.sectors[self.cursor] = data
                self.writes.append(self.cursor)
                self.accept(self.cursor, data)
            return 0, self.cursor, 0, b''
        if op == OP['terrain_verify']:
            expected = struct.unpack('<I', payload)[0]
            data = b''.join(self.sectors.get(arg + i, b'\xff' * SECTOR) for i in range(count))
            ok = crc(data) == expected
            if ok:
                for i in range(count):
                    self.accept(arg + i, self.sectors[arg + i])
            return 0, self.cursor, int(ok), struct.pack('<I', crc(data)) + bytes([ok])
        if op == OP['terrain_commit']:
            _, length, checksum, tag, sha = self.header
            if self.cursor != -(-length // SECTOR) or self.running != checksum:
                return 5, 0, 0, b''
            self.header = (2, length, checksum, tag, sha)
            return 0, 0, 0, self.terrain_info()
        if op == OP['terrain_crc']:
            data = b''.join(self.sectors.get(arg + i, bytes(SECTOR)) for i in range(count))
            return 0, arg, count, struct.pack('<I', crc(data))
        if op == OP['files_status']:
            return 0, 0, len(self.files), struct.pack('<QQIIIQ', 3, 0xAB, len(self.files), 128, 1 << 20, 524_224)
        if op == OP['files_begin']:
            self.writer = (payload.decode(), count, bytearray())
            return 0, 0, count, b''
        if op == OP['files_write']:
            name, length, data = self.writer
            if arg != len(data):
                return 4, 0, 0, b''
            data += payload
            return 0, len(data), 0, b''
        if op == OP['files_commit']:
            name, length, data = self.writer
            if crc(bytes(data)) != count:
                return 5, 0, 0, b''
            self.files[name] = bytes(data)
            return 0, 0, 0, self.entry(0, name)
        if op == OP['files_stat']:
            name = payload.decode()
            return (0, 0, 0, self.entry(0, name)) if name in self.files else (8, 0, 0, b'')
        if op == OP['files_read']:
            data = self.files[payload.decode()]
            return 0, arg, crc(data), data[arg : arg + count]
        if op == OP['files_list']:
            entries = b''.join(self.entry(i, name) for i, name in enumerate(sorted(self.files)))
            return 0, 128, len(self.files), entries
        if op == OP['files_delete']:
            return (0, 0, 0, b'') if self.files.pop(payload.decode(), None) is not None else (8, 0, 0, b'')
        raise AssertionError(op)

    def entry(self, slot, name):
        data = self.files[name]
        return struct.pack('<H', slot) + name.encode().ljust(40, b'\0') + struct.pack('<III', len(data), crc(data), 1)


class FakeTransfer(Transfer):
    def __init__(self, device):
        super().__init__(None)
        self.device = device

    def call(self, name, arg=0, count=0, payload=b'', allow=()):
        status, arg, count, data = self.device.handle(OP[name], arg, count, payload)
        if status and volumes.STATUS[status] not in allow:
            raise volumes.TransferError(name, status)
        return volumes.STATUS[status], arg, count, data


class VolumesTests(unittest.TestCase):
    def pack(self, directory, size=300_001):
        path = Path(directory) / 'chile.pack'
        path.write_bytes(bytes((i * 31 + i // 4096) % 256 for i in range(size)))
        return path

    def test_the_wire_client_finds_its_reply_past_stray_bytes(self):
        host, device = os.pipe()
        reply_r, reply_w = os.pipe()
        try:
            transfer = Transfer(reply_r)
            transfer.fd = reply_r
            sent = []

            def send(data, _deadline):
                sent.append(data)

            transfer._send = send
            os.write(
                reply_w,
                b'log noise C6V'
                + frame(b'C6VP', 0, OP['hello'], 99)  # an earlier request's reply
                + frame(b'C6VP', 0, OP['hello'], 1, 0, 0, b'hello'),
            )
            status, _, _, data = transfer.call('hello')
            self.assertEqual((status, data), ('ok', b'hello'))
            head = sent[0]
            self.assertEqual(head[:4], b'C6VQ')
            self.assertEqual(struct.unpack('<I', head[32:36])[0], crc(head[:32]))
            os.write(reply_w, frame(b'C6VP', 6, OP['terrain_status'], 2))
            with self.assertRaisesRegex(volumes.TransferError, 'stale'):
                transfer.call('terrain_status')
        finally:
            for fd in (host, device, reply_r, reply_w):
                os.close(fd)

    def test_inspect_decodes_volumes_terrain_and_files(self):
        device = FakeDevice()
        result = volumes.inspect(FakeTransfer(device))
        self.assertEqual(result['layout'], 'volumes')
        self.assertEqual([v['name'] for v in result['volumes']], ['RIDES', 'FILES', 'TERRAIN'])
        self.assertEqual(
            result['volumes'][2], {'name': 'TERRAIN', 'kind': 'pack', 'start': 2_623_488, 'sectors': 2_621_440}
        )
        self.assertEqual(result['terrain']['state'], 'empty')
        self.assertEqual(result['unallocated'], 532_479)
        self.assertEqual((result['files']['state'], result['files']['store']), ('ready', '00000000000000ab'))

    def test_terrain_write_streams_commits_and_verifies(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.pack(directory)
            device = FakeDevice()
            status = volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V1', verify=True)
            self.assertEqual(status['state'], 'ready')
            self.assertEqual(status['sha256'], hashlib.sha256(path.read_bytes()).hexdigest())
            data = b''.join(device.sectors[i] for i in range(len(device.sectors)))
            self.assertEqual(data[: path.stat().st_size], path.read_bytes())
            # Installed already: nothing is written again.
            writes = len(device.writes)
            volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V1')
            self.assertEqual(len(device.writes), writes)
            # The same bytes under another tag are another pack: rewritten, with the new tag.
            status = volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V2')
            self.assertGreater(len(device.writes), writes)
            self.assertEqual((status['state'], status['tag']), ('ready', 'CHILE-V2'))

    def test_an_interrupted_terrain_write_resumes_after_what_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.pack(directory)
            device = FakeDevice()
            device.fail_write_at = 300
            with self.assertRaises(volumes.TransferError):
                volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V1')
            written = len(device.writes)
            status = volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V1')
            self.assertEqual(status['state'], 'ready')
            # Sectors 0..255 verified as one chunk; 256 onwards rewritten.
            self.assertEqual(device.writes[written], 256)
            volumes.terrain_verify(FakeTransfer(device), path)

    def test_packs_are_hashed_before_the_timed_session_opens(self):
        order = []

        class Connection:
            def __enter__(self):
                order.append('connect')
                return self

            def __exit__(self, *_):
                return False

        def session(_):
            order.append('session')
            return mock.Mock()

        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.multiple(
                volumes,
                private_path=lambda _: Path(directory),
                UsbConnection=lambda *_: Connection(),
                open_session=session,
                digest_file=lambda _: order.append('hash') or (1, b'', 2),
                terrain_verify=lambda _t, _p, digest: order.append(('verify', digest)),
            ),
        ):
            volumes.run(Namespace(action='terrain-verify', file=Path('pack')), 'port')
        self.assertEqual(order, ['hash', 'connect', 'session', ('verify', (1, b'', 2))])

    def test_verify_reports_a_differing_pack(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.pack(directory)
            device = FakeDevice()
            volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V1')
            device.sectors[5] = bytes(SECTOR)
            with self.assertRaisesRegex(RuntimeError, 'sectors 0..256 differ'):
                volumes.terrain_verify(FakeTransfer(device), path)
            # Installed already: --verify still reads the pack back.
            with self.assertRaisesRegex(RuntimeError, 'sectors 0..256 differ'):
                volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V1', verify=True)
            other = Path(directory) / 'other.pack'
            other.write_bytes(b'x')
            with self.assertRaisesRegex(RuntimeError, 'different file'):
                volumes.terrain_verify(FakeTransfer(device), other)

    def test_a_pack_larger_than_the_volume_is_refused_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.pack(directory, 5000)
            device = FakeDevice(capacity=4096)
            with self.assertRaisesRegex(ValueError, 'exceeds'):
                volumes.terrain_write(FakeTransfer(device), path, 'CHILE-V1')
            self.assertEqual(device.writes, [])
        with self.assertRaises(ValueError):
            volumes.tag_bytes('has space')

    def test_files_put_get_list_delete(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'ftp.json'
            source.write_bytes(b'{"steps": []}' * 700)
            device = FakeDevice()
            transfer = FakeTransfer(device)
            entry = volumes.files_put(transfer, 'workouts/ftp.json', source)
            self.assertEqual(entry['length'], source.stat().st_size)
            self.assertEqual([e['name'] for e in volumes.files_list(transfer)], ['workouts/ftp.json'])
            out = Path(directory) / 'back.json'
            volumes.files_get(transfer, 'workouts/ftp.json', out)
            self.assertEqual(out.read_bytes(), source.read_bytes())
            transfer.call('files_delete', payload=b'workouts/ftp.json')
            self.assertEqual(volumes.files_list(transfer), [])

    def test_a_get_whose_bytes_do_not_match_leaves_no_file(self):
        with tempfile.TemporaryDirectory() as directory:
            device = FakeDevice()
            device.files['a'] = b'abc'
            transfer = FakeTransfer(device)
            original = device.entry

            def wrong(slot, name):
                return original(slot, name)[:46] + struct.pack('<I', 1) + original(slot, name)[50:]

            device.entry = wrong
            out = Path(directory) / 'a'
            with self.assertRaisesRegex(RuntimeError, 'does not match'):
                volumes.files_get(transfer, 'a', out)
            self.assertFalse(out.exists())
            self.assertFalse(Path(f'{out}.partial').exists())


if __name__ == '__main__':
    unittest.main()
