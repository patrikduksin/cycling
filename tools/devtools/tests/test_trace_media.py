import hashlib
import os
import shutil
import struct
import tempfile
import unittest
import zlib
from pathlib import Path
from unittest import mock

from cycling_devtools.device import trace_media, volumes
from cycling_devtools.device.volumes import OP, crc
from test_volumes import FakeDevice, FakeTransfer

SLOT_SIZE = trace_media.SLOT_SIZE
STORE = '00000000000000ab'


def slot(index):
    return bytes([index + 1]) * SLOT_SIZE


class FakeExport:
    """Ride export replies for both journals, as the device's EXPORT commands give them."""

    def __init__(self, internal, rides, store=STORE, export_rides=False):
        self.internal, self.rides, self.store, self.commands = internal, rides, store, []
        self.export_rides = export_rides

    def terminal_command(self, command):
        self.commands.append(command)
        words = command.split()
        bulk = words[1] == 'BULK'
        slots = self.rides if bulk else self.internal
        verb = words[2 if bulk else 1]
        if verb == 'INFO':
            data = f'INFO 1 256 {len(slots)} ready' + (f' {self.store} 3 5777391' if bulk else '')
        else:
            assert not bulk or self.export_rides, 'RIDES is read through the transfer session'
            index = int(words[-1])
            data = f'SLOT {index} {zlib.crc32(slots[index]):08x} {slots[index].hex()}' + (' valid' if bulk else '')
        return dict(id=len(self.commands), status='OK', data=data)


class RidesDevice(FakeDevice):
    """A transfer session that also reads the RIDES log: `rides` maps a store id to its records, None for a sector
    that holds no valid record of it; `asked` lists the first record of each RIDES_READ."""

    def __init__(self, rides=None):
        super().__init__()
        self.rides, self.asked = rides or {}, []

    def handle(self, op, arg, count, payload):
        if op != OP['rides_read']:
            return super().handle(op, arg, count, payload)
        self.asked.append(arg)
        records = self.rides.get(struct.unpack('<Q', payload)[0], [])
        data, sent = b'', 0
        while sent < count and len(data) + 2 + SLOT_SIZE <= 4096:
            record = records[arg + sent] if arg + sent < len(records) else None
            data += struct.pack('<H', 0xFFFF) if record is None else struct.pack('<H', len(record)) + record
            sent += 1
        return 0, arg + sent, sent, data


def sealed(body):
    return body + struct.pack('<I', zlib.crc32(body))


class RewritingDevice(RidesDevice):
    """FILES where `rewritten` gets the bytes `again`, at revision 2, right after a host first stats it."""

    def __init__(self, rewritten, again):
        super().__init__()
        self.revisions, self.rewrite = {}, (rewritten, again)

    def handle(self, op, arg, count, payload):
        reply = super().handle(op, arg, count, payload)
        rewritten, again = self.rewrite
        if op == OP['files_stat'] and payload.decode() == rewritten:
            self.files[rewritten], self.revisions[rewritten] = again, 2
            self.rewrite = (None, None)
        return reply

    def entry(self, slot, name):
        data = self.files[name]
        return (
            struct.pack('<H', slot)
            + name.encode().ljust(40, b'\0')
            + struct.pack('<III', len(data), crc(data), self.revisions.get(name, 1))
        )


class UnreadableDevice(RidesDevice):
    def __init__(self, unreadable):
        super().__init__()
        self.unreadable = set(unreadable)

    def handle(self, op, arg, count, payload):
        if op == OP['files_read'] and payload.decode() in self.unreadable:
            return volumes.STATUS.index('media'), 0, 0, b''
        return super().handle(op, arg, count, payload)


def header(pack=None):
    lines = ['boot 7', 'journal 2', f'rides log {STORE} 3', 'files 00000000000000ab 9']
    lines += ['file workouts/tempo.vw 46 2144df1c 1', 'file bests.dat 42 2144df1c 3']
    if pack:
        lines.append(f'terrain {len(pack)} {hashlib.sha256(pack).hexdigest()} ready CHILE-TERRAIN-V1')
    else:
        lines.append('terrain none')
    return '\n'.join(lines) + '\n'


class TracePullTests(unittest.TestCase):
    def pull(self, directory, export, pack=None, roots=(), device=None):
        device = device or RidesDevice()
        if not device.rides and export.rides:
            device.rides = {int(export.store, 16): list(export.rides)}
        device.store = {'traces/000003-00000007.trace': (1, [b'trace'])}
        device.files = device.files or {'workouts/tempo.vw': b'tempo workout'}
        calls = []

        def tool(command, *args):
            calls.append((command, *args))
            if command == 'media':
                return header(pack)
            out = Path(args[2])
            calls.append(sorted(str(path.relative_to(out)) for path in out.rglob('*')))
            return f'origin {args[0].with_suffix(".origin")}\nfile bests.dat (revision 3): gone from the device\n'

        with mock.patch.object(volumes, 'open_session', return_value=FakeTransfer(device)):
            result = trace_media.pull(
                export, directory=directory, tool=tool, roots=roots, copies=directory / 'terrain' / 'sha256'
            )
        return result, calls

    def test_pull_copies_what_the_header_names_up_to_the_ends_it_recorded(self):
        export = FakeExport([slot(i) for i in range(5)], [slot(10 + i) for i in range(8)])
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            pack = b'terrain pack bytes'
            (directory / 'terrain').mkdir()
            (directory / 'terrain' / 'other.terrain').write_bytes(b'x' * len(pack))
            (directory / 'terrain' / 'chile.terrain').write_bytes(pack)
            result, calls = self.pull(directory, export, pack, roots=(directory / 'terrain',))
            media = directory / '000003-00000007.media'
            self.assertEqual((directory / '000003-00000007.trace').read_bytes(), b'trace')
            self.assertEqual((media / 'internal.slots').read_bytes(), slot(0) + slot(1))
            self.assertEqual((media / 'rides.slots').read_bytes(), slot(10) + slot(11) + slot(12))
            self.assertEqual((media / 'rides.epoch').read_text(), f'store {STORE}\ngeneration 3\ninvalid \n')
            self.assertEqual((media / 'files.list').read_text(), f'store {STORE}\nfile 1 workouts/tempo.vw\n')
            self.assertEqual((media / 'files' / 'workouts' / 'tempo.vw').read_bytes(), b'tempo workout')
            link = media / 'terrain.pack'
            self.assertFalse(Path(os.readlink(link)).is_absolute(), 'a link that holds on another machine')
            self.assertEqual(link.read_bytes(), pack)
            moved = Path(tempfile.mkdtemp(dir=directory.parent))
            shutil.copytree(directory, moved / 'local', symlinks=True)
            self.assertEqual((moved / 'local' / media.relative_to(directory) / 'terrain.pack').read_bytes(), pack)
            shutil.rmtree(moved)
            self.assertEqual(calls[0], ('media', directory / '000003-00000007.trace'))
            self.assertEqual(calls[1][:2], ('rebuild', directory / '000003-00000007.trace'))
            self.assertIn('files/workouts/tempo.vw', calls[2])
            self.assertEqual(result['rebuilt'], ['file bests.dat (revision 3): gone from the device'])
            self.assertEqual(result['origin'], str(directory / '000003-00000007.origin'))
            self.assertNotIn('problems', result)
        self.assertNotIn('EXPORT SLOT 2', export.commands, 'stops at the slots the header names')
        self.assertFalse(any(word in ' '.join(export.commands) for word in ('CLEAR', 'INIT', 'PUT', 'DELETE')))

    def test_a_ride_log_cleared_since_is_not_pulled_and_no_pack_is_linked(self):
        export = FakeExport([slot(0)], [slot(20)], store='00000000000000cd')
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            pack = b'terrain pack bytes'
            self.pull(directory, export, pack, roots=(directory / 'none',))
            media = directory / '000003-00000007.media'
            self.assertEqual((media / 'rides.slots').read_bytes(), b'')
            self.assertIn('store 00000000000000cd', (media / 'rides.epoch').read_text())
            self.assertEqual((media / 'internal.slots').read_bytes(), slot(0), 'all the device holds')
            self.assertFalse((media / 'terrain.pack').exists())
        self.assertFalse(any(command.startswith('EXPORT BULK SLOT') for command in export.commands))

    def test_a_file_written_again_while_it_is_pulled_is_not_paired_with_a_revision(self):
        export = FakeExport([slot(0), slot(1)], [slot(10 + i) for i in range(3)])
        before, after = sealed(b'tempo workout A'), sealed(b'tempo workout B')
        self.assertEqual(zlib.crc32(before), zlib.crc32(after), 'the whole-file CRC32 cannot tell them apart')
        device = RewritingDevice('workouts/tempo.vw', after)
        device.files = {'workouts/tempo.vw': before}
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            result, calls = self.pull(directory, export, device=device)
            media = directory / '000003-00000007.media'
            self.assertEqual((media / 'files.list').read_text(), f'store {STORE}\nunread workouts/tempo.vw\n')
            self.assertFalse((media / 'files' / 'workouts' / 'tempo.vw').exists())
            self.assertEqual(
                result['problems'], ['file workouts/tempo.vw: written again while it was read (revision 1)']
            )
            self.assertEqual(calls[1][:2], ('rebuild', directory / '000003-00000007.trace'))

    def test_a_file_that_does_not_read_is_listed_unread_and_the_pull_goes_on(self):
        export = FakeExport([slot(0), slot(1)], [slot(10 + i) for i in range(3)])
        device = UnreadableDevice({'workouts/tempo.vw'})
        device.files = {'workouts/tempo.vw': b'tempo workout', 'bests.dat': b'bests'}
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            result, calls = self.pull(directory, export, device=device)
            media = directory / '000003-00000007.media'
            self.assertEqual(
                (media / 'files.list').read_text(), f'store {STORE}\nunread workouts/tempo.vw\nfile 1 bests.dat\n'
            )
            self.assertEqual((media / 'files' / 'bests.dat').read_bytes(), b'bests')
            self.assertEqual(result['problems'], ['file workouts/tempo.vw: files_read failed: media'])
            self.assertEqual(calls[1][:2], ('rebuild', directory / '000003-00000007.trace'))

    def test_rides_are_read_through_the_session_and_a_later_pull_reads_only_new_records(self):
        rides = [slot(10 + i) for i in range(40)]
        records = [*rides[:7], None, rides[8], b'short', *rides[10:]]
        device = RidesDevice({int(STORE, 16): records})
        export = FakeExport([slot(0), slot(1)], rides)
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            with mock.patch.object(trace_media, 'header_media', return_value=self.media(head=30)):
                self.pull(directory, export, device=device)
            media = directory / '000003-00000007.media'
            zero = bytes(SLOT_SIZE)
            expected = b''.join(rides[:7]) + zero + rides[8] + zero + b''.join(rides[10:30])
            self.assertEqual((media / 'rides.slots').read_bytes(), expected)
            self.assertEqual((media / 'rides.epoch').read_text(), f'store {STORE}\ngeneration 3\ninvalid 7 9\n')
            self.assertEqual(len(device.asked), 2, 'records come in batches')

            device.asked = []
            with mock.patch.object(trace_media, 'header_media', return_value=self.media(head=40)):
                self.pull(directory, export, device=device)
            self.assertEqual(device.asked, [29, 30], 'the cached tail, then only the records after it')
            self.assertEqual((media / 'rides.slots').read_bytes(), expected + b''.join(rides[30:40]))
            self.assertEqual((media / 'rides.epoch').read_text(), f'store {STORE}\ngeneration 3\ninvalid 7 9\n')

            device.asked = []
            with mock.patch.object(trace_media, 'header_media', return_value=self.media(head=20)):
                self.pull(directory, export, device=device)
            self.assertEqual(device.asked, [19], 'a shorter pull checks its own last record')
            self.assertEqual((media / 'rides.slots').read_bytes(), expected[: 20 * SLOT_SIZE])

        self.assertFalse(any('BULK SLOT' in command for command in export.commands))

    def test_a_cache_whose_tail_the_device_no_longer_holds_is_read_again(self):
        rides = [slot(10 + i) for i in range(20)]
        device = RidesDevice({int(STORE, 16): list(rides)})
        export = FakeExport([], rides)
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            with mock.patch.object(trace_media, 'header_media', return_value=self.media(head=20)):
                self.pull(directory, export, device=device)
                device.rides[int(STORE, 16)][19] = slot(77)
                device.asked = []
                self.pull(directory, export, device=device)
            self.assertEqual(device.asked[:2], [19, 0])
            media = directory / '000003-00000007.media'
            self.assertEqual((media / 'rides.slots').read_bytes()[-SLOT_SIZE:], slot(77))

    def test_firmware_from_before_rides_read_gets_rides_through_ride_export(self):
        rides = [slot(10 + i) for i in range(5)]
        export = FakeExport([], rides, export_rides=True)
        device = RidesDevice()
        device.handle = lambda op, *args: (
            (volumes.STATUS.index('invalid'), 0, 0, b'')
            if op == OP['rides_read']
            else FakeDevice.handle(device, op, *args)
        )
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            with mock.patch.object(trace_media, 'header_media', return_value=self.media(head=4)):
                result, _ = self.pull(directory, export, device=device)
            media = directory / '000003-00000007.media'
            self.assertEqual((media / 'rides.slots').read_bytes(), b''.join(rides[:4]))
            self.assertEqual((media / 'rides.epoch').read_text(), f'store {STORE}\ngeneration 3\ninvalid \n')
            self.assertNotIn('problems', result)
        self.assertIn('EXPORT BULK SLOT 3', export.commands)
        self.assertNotIn('EXPORT BULK SLOT 4', export.commands)

    def media(self, head):
        media = trace_media.header_media(header())
        media['journal'] = None
        media['rides']['head'] = head
        return media

    def test_a_file_name_that_leaves_files_is_refused(self):
        for name in ('../escape', '/etc/passwd', 'a/../../b'):
            with self.assertRaises(ValueError, msg=name):
                trace_media.safe_name(name)
        self.assertEqual(trace_media.safe_name('rides/facts.tbl'), Path('rides/facts.tbl'))

    def test_header_media_lines_read_as_a_dict(self):
        media = trace_media.header_media('boot 2\njournal none\nrides none\nfiles none\nterrain none\n')
        self.assertEqual(media, {'journal': None, 'rides': None, 'files': None, 'terrain': None})
        media = trace_media.header_media(header(b'pack'))
        self.assertEqual(media['rides'], {'store': STORE, 'head': 3})
        self.assertEqual([entry['name'] for entry in media['files']['entries']], ['workouts/tempo.vw', 'bests.dat'])
        self.assertEqual(media['terrain']['tag'], 'CHILE-TERRAIN-V1')
        media = trace_media.header_media(f'rides uncertain {STORE} 5\n')
        self.assertEqual(media['rides'], {'store': STORE, 'head': 5}, 'its records are pulled')
        for line in ('rides unusable', 'rides uninitialized'):
            self.assertIsNone(trace_media.header_media(line)['rides'], line)
        media = trace_media.header_media('journal unusable\nfiles unreadable volume\nterrain unreadable card\n')
        self.assertEqual(media, {'journal': None, 'rides': None, 'files': None, 'terrain': None}, 'nothing to pull')


if __name__ == '__main__':
    unittest.main()
