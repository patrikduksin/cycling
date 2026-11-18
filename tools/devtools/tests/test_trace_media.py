import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from cycling_devtools.device import trace_media, volumes
from test_volumes import FakeDevice, FakeTransfer

MARKS = [
    {
        'type': 'entry',
        'session_ms': 42410,
        'ride_ms': 35000,
        'kind': 'mark',
        'text': '1 pressed 00:40.900 on ride/main',
        'repeat': False,
    },
    {
        'type': 'entry',
        'session_ms': 70020,
        'ride_ms': 62000,
        'kind': 'mark',
        'text': '2 pressed 01:08.510 on ride/stats',
        'repeat': False,
    },
]


BUILD = 'ab' * 20


def recorded(directory, version=5, build=BUILD):
    """A trace whose header names its format and the build that recorded it."""
    trace = directory / 'ride.trace'
    trace.write_bytes(bytes([0, 0, version]) + bytes.fromhex(build) + b'a ride')
    return trace


def unlaunchable(directory):
    """A twin that is there but cannot start, as one whose loader is gone."""
    binary = directory / 'trace'
    binary.write_text('#!/nonexistent/ld-linux.so\n')
    binary.chmod(0o755)
    return binary


def tool(directory, script):
    """A trace tool that runs `script` with its arguments as $@."""
    binary = directory / 'trace'
    binary.write_text(f'#!/bin/sh\n{script}\n')
    binary.chmod(0o755)
    return binary


class TracePullTests(unittest.TestCase):
    def test_pull_copies_the_trace_and_nothing_else(self):
        device = FakeDevice()
        device.store = {'traces/000003-00000007.trace': (1, [b'trace'])}
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(volumes, 'open_session', return_value=FakeTransfer(device)),
        ):
            directory = Path(directory)
            result = trace_media.pull(None, directory=directory)
            trace = directory / '000003-00000007.trace'
            self.assertEqual(trace.read_bytes(), b'trace')
            self.assertEqual(sorted(path.name for path in directory.iterdir()), [trace.name])
        self.assertEqual(result['path'], str(trace))
        self.assertEqual((result['bytes'], result['sha256']), (5, trace_media.hashlib.sha256(b'trace').hexdigest()))
        self.assertNotIn('marks', result, 'the pull only copies')
        self.assertNotIn('faithful', result, 'the pull only copies')
        self.assertIn('trace_session', result['timing'])


class JudgeTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.directory = Path(scratch.name)
        self.twins = []

    def judge(self, trace, binary=None, missing=None):
        def twin_for(build, root):
            self.twins.append(build)
            if missing:
                raise trace_media.twin.NotArchived(missing)
            return binary

        result = {'path': str(trace), 'timing': {}}
        checks = []
        trace_media.judge(
            result,
            marks_of=lambda path, build: trace_media.recorded_marks(path, build, twin_for=twin_for),
            check=lambda path, build: checks.append((path, build)) or {'ok': True, 'ended': None, 'exclusions': []},
        )
        return result, checks

    def test_the_marks_are_listed_by_the_trace_tool_of_the_build_that_recorded_the_trace(self):
        trace = recorded(self.directory, version=5)
        found = json.dumps({'found': MARKS, 'total': 2})
        listed = tool(self.directory, f'[ "$1 $3 $4 $5 $6" = "find mark --limit all --json" ] && echo \'{found}\'')
        result, checks = self.judge(trace, binary=listed)
        self.assertEqual(result['marks'], MARKS)
        self.assertEqual(self.twins, [BUILD])
        self.assertEqual(checks, [(trace, BUILD)])
        self.assertEqual(result['faithful'], {'ok': True, 'ended': None, 'exclusions': []})

    def test_marks_no_tool_can_list_are_skipped_and_the_pull_still_succeeds(self):
        trace = recorded(self.directory, version=6)
        result, _ = self.judge(trace, missing='build abababababab is not archived on this machine')
        self.assertEqual(result['marks'], {'skipped': 'build abababababab is not archived on this machine'})
        self.assertEqual(result['path'], str(trace))
        self.assertNotIn('error', result)
        self.assertEqual(
            trace_media.marks_text(result['marks']), 'marks: unread, build abababababab is not archived on this machine'
        )

        refusing = tool(self.directory, "echo 'error: unrecognized subcommand find' >&2; exit 2")
        result, _ = self.judge(trace, binary=refusing)
        self.assertEqual(
            result['marks'],
            {'skipped': f'the trace tool of build {BUILD[:12]} lists no marks: error: unrecognized subcommand find'},
        )

    def test_a_twin_that_cannot_start_skips_the_marks(self):
        result, _ = self.judge(recorded(self.directory), binary=unlaunchable(self.directory))
        self.assertTrue(result['marks']['skipped'].startswith(f'the twin of build {BUILD[:12]} did not start: '))

    def test_a_pull_under_json_whose_twin_cannot_start_still_prints_one_document(self):
        trace = recorded(self.directory)
        binary = unlaunchable(self.directory)
        result = {'path': str(trace), 'timing': {}}
        with (
            mock.patch.object(trace_media, 'recorded_twin', lambda twin_for, build, root: binary),
            mock.patch.object(trace_media.workspace, 'main_checkout', lambda root: self.directory),
            mock.patch('sys.stdout', new_callable=io.StringIO) as out,
        ):
            volumes.finish_pull(result, as_json=True)
        printed = json.loads(out.getvalue())
        self.assertIn('did not start', printed['marks']['skipped'])
        self.assertIn('did not start', printed['faithful']['skipped'])
        self.assertNotIn('error', printed)

    def test_a_judge_that_fails_under_json_is_one_error_document(self):
        result = {'path': str(recorded(self.directory)), 'timing': {}}
        with (
            mock.patch.object(trace_media, 'judge', side_effect=ValueError('the twin cache is unreadable')),
            mock.patch('sys.stdout', new_callable=io.StringIO) as out,
            self.assertRaises(SystemExit) as exit,
        ):
            volumes.finish_pull(result, as_json=True)
        self.assertEqual(exit.exception.code, 2)
        self.assertEqual(json.loads(out.getvalue())['error'], 'the twin cache is unreadable')

    def test_a_trace_that_names_no_build_is_neither_listed_nor_checked(self):
        trace = recorded(self.directory, build='00' * 20)
        result, checks = self.judge(trace)
        self.assertEqual(result['marks'], {'skipped': 'the trace names no build'})
        self.assertEqual(result['faithful'], {'skipped': 'the trace names no build'})
        self.assertEqual((self.twins, checks), ([], []))

    def test_the_marks_print_one_line_each(self):
        self.assertEqual(
            trace_media.marks_text(MARKS),
            'marks:\n  mark 1 pressed 00:40.900 on ride/main\n  mark 2 pressed 01:08.510 on ride/stats',
        )
        self.assertEqual(trace_media.marks_text([]), 'marks: none')


class TerrainPackTests(unittest.TestCase):
    def test_the_faithful_replay_reads_packs_from_the_primary_checkout(self):
        with tempfile.TemporaryDirectory() as scratch:
            primary = Path(scratch)
            echo = tool(primary, 'echo "{\\"packs\\": \\"$CYCLING_TERRAIN_PACKS\\"}"')
            with mock.patch.object(trace_media.workspace, 'main_checkout', lambda root: primary):
                with mock.patch.dict(trace_media.os.environ, {}, clear=False) as env:
                    env.pop('CYCLING_TERRAIN_PACKS', None)
                    self.assertEqual(
                        trace_media.replay_faithful(echo, primary / 'ride.trace', root=primary),
                        {'packs': str(primary / '.local' / 'terrain')},
                    )
                    env['CYCLING_TERRAIN_PACKS'] = '/elsewhere'
                    self.assertEqual(
                        trace_media.replay_faithful(echo, primary / 'ride.trace', root=primary),
                        {'packs': '/elsewhere'},
                    )


def replayed(ok, ended=None, verdict='the replay re-recorded the trace byte for byte'):
    proved = {'ok': ok, 'physical': 9, 'semantic': 1, 'ended': ended, 'exclusions': ['8 TERRAIN reads']}
    return {'divergence': {'ok': ok, 'verdict': verdict}, 'faithful': proved}


class FaithfulCheckTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.trace = self.root / 'ride.trace'
        self.trace.write_bytes(b'a ride')
        self.replays = []
        patched = mock.patch.object(trace_media.workspace, 'main_checkout', lambda root: self.root)
        patched.start()
        self.addCleanup(patched.stop)

    def check(self, outcome, twin_for=None):
        def replay(binary, trace, root):
            self.replays.append((binary, trace))
            return outcome

        return trace_media.faithful(
            self.trace,
            BUILD,
            root=self.root,
            twin_for=twin_for or (lambda build, root: Path('/twin')),
            check=lambda binary, build, root: None,
            replay=replay,
        )

    def test_a_verdict_is_cached_by_the_trace_and_its_build(self):
        first = self.check(replayed(True))
        self.assertEqual(first, {'ok': True, 'ended': None, 'exclusions': ['8 TERRAIN reads']})
        self.assertEqual(self.check(replayed(False)), first, 'read from the cache')
        self.assertEqual(len(self.replays), 1)
        digest = trace_media.hashlib.sha256(b'a ride').hexdigest()
        self.assertTrue((self.root / '.local' / 'traces' / '.faithful' / f'{digest}-{BUILD}.json').exists())

    def test_a_divergence_is_a_recording_bug_and_the_pull_leads_with_it(self):
        bug = 'a recording bug at 12:30.250: pass at 750250 ms: recorded A, replayed B'
        checked = self.check(replayed(False, ended='12:30.250: recorded A', verdict=bug))
        self.assertEqual(checked['ok'], False)
        self.assertEqual(trace_media.recording_bug({'faithful': checked}), f'a recording bug: {bug}')
        self.assertIsNone(trace_media.recording_bug({'faithful': {'skipped': 'not archived'}}))

    def test_an_unarchived_build_is_skipped_and_says_why(self):
        def missing(build, root):
            raise trace_media.twin.NotArchived('build abababababab is not archived on this machine')

        self.assertEqual(
            self.check(replayed(True), twin_for=missing),
            {'skipped': 'build abababababab is not archived on this machine'},
        )
        self.assertEqual(self.replays, [])

    def test_a_replay_that_leaves_the_recorded_world_at_boot_is_a_recording_bug(self):
        left = 'at boot, left the recorded world at 00:00.010: unknown media Data 0+64'
        checked = self.check({'error': f'trace: {left}', 'left': left})
        self.assertEqual(checked, {'ok': False, 'ended': left, 'exclusions': []})
        self.assertEqual(trace_media.recording_bug({'faithful': checked}), f'a recording bug: {left}')

    def test_a_missing_terrain_pack_is_its_own_skip(self):
        checked = self.check({'error': 'the session read the TERRAIN pack ...', 'no_pack': 'cd' * 32})
        self.assertEqual(checked, {'skipped': f'no TERRAIN pack {"cd" * 32}'})
        self.assertEqual(self.check(replayed(True))['ok'], True, 'a skip is not cached')

    def test_a_twin_build_that_names_no_binary_is_skipped_and_the_pull_goes_on(self):
        def nameless(build, root):
            raise RuntimeError('cargo built no trace binary in /scratch')

        self.assertEqual(
            self.check(replayed(True), twin_for=nameless),
            {'skipped': f'the twin of build {BUILD[:12]} did not build: cargo built no trace binary in /scratch'},
        )

    def test_a_twin_that_cannot_start_is_skipped_and_not_cached(self):
        binary = unlaunchable(self.root)
        for check in (trace_media.twin.faithful_to, lambda binary, build, root: None):
            checked = trace_media.faithful(
                self.trace, BUILD, root=self.root, twin_for=lambda build, root: binary, check=check
            )
            self.assertTrue(checked['skipped'].startswith(f'the twin of build {BUILD[:12]} did not start: '))
        self.assertFalse((self.root / '.local' / 'traces' / '.faithful').exists())

    def test_a_build_older_than_the_faithful_mode_is_skipped_unbuilt(self):
        def old(build, root):
            raise trace_media.twin.Incapable('the trace tool of 0123456789ab has no --mode faithful')

        self.assertEqual(
            self.check(replayed(True), twin_for=old),
            {'skipped': 'the trace tool of 0123456789ab has no --mode faithful'},
        )
        self.assertEqual(self.replays, [])

    def test_a_twin_that_cannot_replay_the_trace_skips_and_is_not_cached(self):
        checked = self.check({'error': '--mode is auto, stimulus or strict, not "faithful"'})
        self.assertIn('skipped', checked)
        self.assertEqual(self.check(replayed(True))['ok'], True)


if __name__ == '__main__':
    unittest.main()
