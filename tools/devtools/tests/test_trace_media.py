import hashlib
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

    def __init__(self, internal, rides, store=STORE):
        self.internal, self.rides, self.store, self.commands = internal, rides, store, []

    def terminal_command(self, command):
        self.commands.append(command)
        words = command.split()
        bulk = words[1] == 'BULK'
        slots = self.rides if bulk else self.internal
        verb = words[2 if bulk else 1]
        if verb == 'INFO':
            data = f'INFO 1 256 {len(slots)} ready' + (f' {self.store} 3 5777391' if bulk else '')
        else:
            index = int(words[-1])
            data = f'SLOT {index} {zlib.crc32(slots[index]):08x} {slots[index].hex()}' + (' valid' if bulk else '')
        return dict(id=len(self.commands), status='OK', data=data)


def sealed(body):
    """`body` followed by its CRC32, as a workout or the bests end."""
    return body + struct.pack('<I', zlib.crc32(body))


class RewritingDevice(FakeDevice):
    """FILES with revisions, where a write of `name` to `again` lands right after the host first stats it,
    and reads of the files in `unreadable` fail on the medium."""

    def __init__(self, name=None, again=None, unreadable=()):
        super().__init__()
        self.revisions, self.rewrite, self.unreadable_files = {}, (name, again), set(unreadable)

    def handle(self, op, arg, count, payload):
        if op == OP['files_read'] and payload.decode() in self.unreadable_files:
            return volumes.STATUS.index('media'), 0, 0, b''
        reply = super().handle(op, arg, count, payload)
        name, again = self.rewrite
        if op == OP['files_stat'] and payload.decode() == name:
            self.files[name], self.revisions[name] = again, 2
            self.rewrite = (None, None)
        return reply

    def entry(self, slot, name):
        data = self.files[name]
        return (
            struct.pack('<H', slot)
            + name.encode().ljust(40, b'\0')
            + struct.pack('<III', len(data), crc(data), self.revisions.get(name, 1))
        )


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
        device = device or FakeDevice()
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
            result = trace_media.pull(export, directory=directory, tool=tool, roots=roots)
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
            self.assertEqual((media / 'terrain.pack').resolve(), (directory / 'terrain' / 'chile.terrain').resolve())
            self.assertEqual(calls[0], ('media', directory / '000003-00000007.trace'))
            self.assertEqual(calls[1][:2], ('rebuild', directory / '000003-00000007.trace'))
            self.assertIn('files/workouts/tempo.vw', calls[2])
            self.assertEqual(result['rebuilt'], ['file bests.dat (revision 3): gone from the device'])
            self.assertEqual(result['origin'], str(directory / '000003-00000007.origin'))
            self.assertNotIn('problems', result)
        self.assertNotIn('EXPORT SLOT 2', export.commands, 'stops at the slots the header names')
        self.assertNotIn('EXPORT BULK SLOT 3', export.commands)
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
        device = RewritingDevice(unreadable={'workouts/tempo.vw'})
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


if __name__ == '__main__':
    unittest.main()
