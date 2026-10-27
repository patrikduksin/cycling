"""Remote development tools: device task snapshots, crash decoding and verify."""

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from cycling_devtools import verify
from cycling_devtools.device import remote
from cycling_devtools.terminal import backtrace
from workspace_fixture import init_checkout

IDENTITY = ['-c', 'user.name=t', '-c', 'user.email=t@t', '-c', 'commit.gpgsign=false']


def commit(root, message='base'):
    subprocess.run(['git', '-C', str(root), 'add', '-A'], check=True)
    subprocess.run(['git', '-C', str(root), *IDENTITY, 'commit', '-q', '-m', message], check=True)


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.root = init_checkout(tempfile.mkdtemp())
        (self.root / 'tracked.txt').write_text('one\n')
        commit(self.root)
        self.patch = patch.object(remote, 'ROOT', self.root)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.root), *args], capture_output=True, text=True).stdout.strip()

    def test_clean_tree_is_head(self):
        self.assertEqual(remote.snapshot(), self.git('rev-parse', 'HEAD'))

    def test_edits_and_untracked_files_are_in_the_snapshot_but_not_the_index(self):
        (self.root / 'tracked.txt').write_text('two\n')
        (self.root / 'new.txt').write_text('new\n')
        (self.root / '.local' / 'private.bin').write_text('never sent\n')
        sha = remote.snapshot()
        self.assertEqual(self.git('rev-parse', f'{sha}^'), self.git('rev-parse', 'HEAD'))
        self.assertEqual(self.git('show', f'{sha}:tracked.txt'), 'two')
        self.assertEqual(self.git('show', f'{sha}:new.txt'), 'new')
        self.assertNotIn('.local', self.git('ls-tree', '-r', '--name-only', sha))
        self.assertEqual(self.git('status', '--porcelain'), 'M tracked.txt\n?? new.txt'.strip())


class RemoteScriptTests(unittest.TestCase):
    def test_task_settings_travel_but_machine_settings_do_not(self):
        env = {
            'CYCLING_SDK': '1',
            'CYCLING_DEVICE_HOST': 'c606-host',
            'CYCLING_ESP_TOOLCHAIN': '/nix/store/x',
            'HOME': '/home/me',
        }
        with patch.dict(remote.os.environ, env, clear=True):
            script = remote.run('.local/remote/a', 'logs', ['STATUS', 'a b'])
        self.assertIn("env CYCLING_SDK=1 cycling logs STATUS 'a b'", script)
        self.assertNotIn('CYCLING_DEVICE_HOST', script)
        self.assertNotIn('/nix/store', script)

    def test_build_is_refused_as_a_device_task(self):
        with patch.dict(remote.os.environ, {'CYCLING_DEVICE_HOST': 'h'}), self.assertRaises(SystemExit) as raised:
            remote.main(['flash-image'])
        self.assertIn('cycling device flash', str(raised.exception))


class BacktraceTests(unittest.TestCase):
    def test_code_addresses_are_annotated_and_data_addresses_ignored(self):
        output = (
            '0x42001234: vana::ride::step at /src/ride.rs:10\n'
            ' (inlined by) vana::ride::run at /src/ride.rs:40\n'
            '0x40379abc: ?? ??:0\n'
        )
        log = ['panicked at src/ride.rs:10\n', 'Backtrace: 0x42001234:0x3fc9a000 0x40379abc:0x3fc9a020\n']
        with patch.object(backtrace, 'addr2line', return_value=Path('addr2line')):
            with patch.object(backtrace.subprocess, 'run') as run:
                run.return_value.stdout = output
                decoded = list(backtrace.decode(log, Path('cycling.elf')))
        addresses = run.call_args.args[0][-2:]
        self.assertEqual(addresses, ['0x42001234', '0x40379abc'])
        self.assertEqual(
            decoded,
            [
                'panicked at src/ride.rs:10',
                'Backtrace: 0x42001234:0x3fc9a000 0x40379abc:0x3fc9a020',
                '    0x42001234 vana::ride::step at /src/ride.rs:10',
                '        (inlined by) vana::ride::run at /src/ride.rs:40',
            ],
        )

    def test_a_crash_record_picks_the_elf_it_names_and_checks_its_commit(self):
        reply = [
            '{"data": "kind=panic commit=15178ed1119a elf=15178ed1119a-dirty-sdk1-h1 pcs=0x42001234,0x42005678"}\n'
        ]
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory)
            self.assertEqual(
                backtrace.choose_elf(reply, archive), (backtrace.ROOT / '.local/cycling.elf', '15178ed1119a')
            )
            (archive / '15178ed1119a-sdk1-h1.elf').write_bytes(b'15178ed1119a')
            named = archive / '15178ed1119a-dirty-sdk1-h1.elf'
            named.write_bytes(b'\0commit 15178ed1119a\0')
            self.assertEqual(backtrace.choose_elf(reply, archive), (named, '15178ed1119a'))
            self.assertTrue(backtrace.holds_commit(named, '15178ed1119a'))
            self.assertFalse(backtrace.holds_commit(named, '0123456789ab'))
        with patch.object(backtrace, 'addr2line', return_value=Path('addr2line')):
            with patch.object(backtrace.subprocess, 'run') as run:
                run.return_value.stdout = ''
                list(backtrace.decode(reply, Path('cycling.elf')))
        self.assertEqual(run.call_args.args[0][-2:], ['0x42001234', '0x42005678'])


class VerifyTests(unittest.TestCase):
    def test_a_failed_step_stops_its_lane_only(self):
        lane = [verify.Step('first', 'exit 3'), verify.Step('second', 'true')]
        with tempfile.TemporaryDirectory() as logs, patch.object(verify, 'ROOT', Path(logs)):
            results = verify.run_lane(lane, Path(logs))
            self.assertEqual([(r.step.name, r.ok) for r in results], [('first', False)])
            self.assertIn('$ exit 3', results[0].log.read_text())

    def test_gate_builds_never_share_an_image(self):
        builds = [step for lane in verify.LANES['full'] for step in lane if step.name.startswith('build-')]
        envs = [dict(step.env) for step in builds]
        self.assertEqual(len({env['CYCLING_IMAGE_DIR'] for env in envs}), 4)
        self.assertNotIn('.local', {env['CYCLING_IMAGE_DIR'] for env in envs})

    def test_every_firmware_variant_is_in_the_full_gate(self):
        names = {step.name for lane in verify.LANES['full'] for step in lane}
        self.assertTrue({'build-sdk0-h0', 'build-sdk0-h1', 'build-sdk1-h0', 'build-sdk1-h1'} <= names)
        self.assertTrue({'test', 'check', 'simulate'} <= names)


if __name__ == '__main__':
    unittest.main()
