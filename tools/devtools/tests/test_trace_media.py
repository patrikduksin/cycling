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


class TracePullTests(unittest.TestCase):
    def pull(self, directory, marks=()):
        device = FakeDevice()
        device.store = {'traces/000003-00000007.trace': (1, [b'trace'])}
        calls = []

        def tool(command, *args):
            calls.append((command, *args))
            if command == 'summary':
                return json.dumps({'header': {'build': BUILD}})
            return json.dumps({'found': list(marks), 'total': len(marks)})

        def check(path, build):
            calls.append(('faithful', path, build))
            return {'ok': True, 'ended': None, 'exclusions': []}

        with mock.patch.object(volumes, 'open_session', return_value=FakeTransfer(device)):
            result = trace_media.pull(None, directory=directory, tool=tool, check=check)
        return result, calls

    def test_pull_copies_the_trace_and_nothing_else(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            result, calls = self.pull(directory)
            trace = directory / '000003-00000007.trace'
            self.assertEqual(trace.read_bytes(), b'trace')
            self.assertEqual(sorted(path.name for path in directory.iterdir()), [trace.name])
            self.assertEqual(
                calls,
                [
                    ('find', trace, 'mark', '--limit', 'all', '--json'),
                    ('summary', trace, '--json'),
                    ('faithful', trace, BUILD),
                ],
            )
            self.assertEqual(result['faithful'], {'ok': True, 'ended': None, 'exclusions': []})
            self.assertEqual(result['path'], str(trace))
            self.assertEqual(result['marks'], [])
            self.assertIn('trace_session', result['timing'])

    def test_the_pull_keeps_every_mark_the_rider_made(self):
        with tempfile.TemporaryDirectory() as directory:
            result, _ = self.pull(Path(directory), marks=MARKS)
        self.assertEqual(result['marks'], MARKS)
        self.assertEqual(
            trace_media.marks_text(result['marks']),
            'marks:\n  mark 1 pressed 00:40.900 on ride/main\n  mark 2 pressed 01:08.510 on ride/stats',
        )
        self.assertEqual(trace_media.marks_text([]), 'marks: none')


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
        def replay(binary, trace):
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
