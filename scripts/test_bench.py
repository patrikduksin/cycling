import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import bench


class Clock:
    now = 0.0
    def monotonic(self): return self.now
    def sleep(self, seconds): self.now += seconds


class BenchTests(unittest.TestCase):
    def test_six_faces_have_separate_five_second_prompts(self):
        faces = [(second, action) for second, action in bench.STEPS if 60 <= second < 90]
        self.assertEqual([second for second, _ in faces], list(range(60, 90, 5)))
        for (_, action), face in zip(faces, ('screen up', 'left', 'right', 'top', 'bottom', 'screen down')):
            self.assertIn(face, action)
        self.assertEqual(bench.STEPS[-1][0], 130)

    def run_fake(self, interrupt=False):
        opened, closed, commands = [], [], []
        clock = Clock()
        class Connection:
            def __init__(self, port, _): self.port = port
            def __enter__(self): opened.append(self.port); return self
            def __exit__(self, *_): closed.append(self.port)
            def terminal_command(self, command, **_):
                commands.append(command)
                if len(commands) == 2:
                    if interrupt: raise KeyboardInterrupt()
                    raise OSError('device renumbered')
                return {'status': 'OK', 'data': ''}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / '.local' / 'bench'
            with patch.object(bench, 'ROOT', root), patch.object(bench, 'UsbConnection', Connection), \
                 patch.object(bench, 'resolve_port', side_effect=[Path('/dev/ttyACM0'), Path('/dev/ttyACM3')]), \
                 patch.object(bench.time, 'monotonic', clock.monotonic), patch.object(bench.time, 'sleep', clock.sleep), \
                 contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                result = bench.collect(output, '/dev/ttyACM0', seconds=.5)
            summary = json.loads((output / 'summary.json').read_text())
            observations = [json.loads(line) for line in (output / 'observations.jsonl').read_text().splitlines()]
        self.assertTrue(set(commands) <= set(bench.COMMANDS))
        self.assertEqual(opened, closed)
        return result, summary, observations, opened

    def test_reconnect_rediscovers_backed_up_device_after_renumber(self):
        result, summary, observations, opened = self.run_fake()
        self.assertEqual(result, 0)
        self.assertEqual(opened, [Path('/dev/ttyACM0'), Path('/dev/ttyACM3')])
        self.assertEqual(summary['transport_gaps'], 1)
        self.assertGreater(summary['samples'], 1)
        self.assertTrue(any(row['type'] == 'gap' for row in observations))

    def test_unverified_port_is_never_opened(self):
        clock = Clock()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / '.local' / 'bench'
            with patch.object(bench, 'ROOT', root), patch.object(bench, 'UsbConnection') as connection, \
                 patch.object(bench, 'resolve_port', return_value=Path('/dev/ttyACM3')), \
                 patch.object(bench.time, 'monotonic', clock.monotonic), patch.object(bench.time, 'sleep', clock.sleep), \
                 contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                bench.collect(output, '/dev/ttyACM0', seconds=.4)
            connection.assert_not_called()
            summary = json.loads((output / 'summary.json').read_text())
            self.assertEqual(summary['samples'], 0)
            self.assertEqual(summary['transport_gaps'], 1)

    def test_interrupt_closes_reader_and_preserves_summary_and_samples(self):
        result, summary, observations, _ = self.run_fake(interrupt=True)
        self.assertEqual(result, 130)
        self.assertEqual(summary['status'], 'interrupted')
        self.assertEqual(summary['samples'], 1)
        self.assertEqual(observations[-1]['type'], 'interrupted')


if __name__ == '__main__':
    unittest.main()
