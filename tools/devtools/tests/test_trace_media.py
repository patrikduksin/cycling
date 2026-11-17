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


class TracePullTests(unittest.TestCase):
    def pull(self, directory, marks=()):
        device = FakeDevice()
        device.store = {'traces/000003-00000007.trace': (1, [b'trace'])}
        calls = []

        def tool(command, *args):
            calls.append((command, *args))
            return json.dumps({'found': list(marks), 'total': len(marks)})

        with mock.patch.object(volumes, 'open_session', return_value=FakeTransfer(device)):
            result = trace_media.pull(None, directory=directory, tool=tool)
        return result, calls

    def test_pull_copies_the_trace_and_nothing_else(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            result, calls = self.pull(directory)
            trace = directory / '000003-00000007.trace'
            self.assertEqual(trace.read_bytes(), b'trace')
            self.assertEqual(sorted(path.name for path in directory.iterdir()), [trace.name])
            self.assertEqual(calls, [('find', trace, 'mark', '--limit', 'all', '--json')])
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


if __name__ == '__main__':
    unittest.main()
