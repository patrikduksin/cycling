import tempfile
import unittest
from pathlib import Path

from cycling_devtools.harness.latency import measure
from cycling_devtools.harness.runner import Runner


class FakeDevice:
    """A harness whose every delivered press is followed by one timed frame."""

    virtual = True

    def __init__(self, latencies, timed=True, dim_ms=None, touched_at=()):
        self.latencies, self.timed, self.dim_ms = list(latencies), timed, dim_ms
        # Presses (1-based, setup included) a hand on the device follows with a physical edge.
        self.touched_at, self.input_events = set(touched_at), 0
        self.commands, self.sent, self.request_id = [], [], 1
        self.uptime, self.submissions, self.presses, self.last = 0, 0, 0, 0
        self.active, self.wakes = 0, 0

    def dimmed(self):
        return self.dim_ms is not None and self.uptime - self.active >= self.dim_ms

    def delay(self, seconds):
        # Two idle frames a second, counted from uptime so short waits add up.
        self.uptime += round(seconds * 1000)
        self.submissions = self.uptime // 500

    def terminal_command(self, command, **_):
        self.commands.append(command)
        self.sent.append(self.uptime)
        self.request_id += 1
        words = command.split()
        common = 'boot=9 session=5'
        if command == 'STATUS':
            timing = f'press_frame_ms={self.last} press_frames={self.presses} display_ms=13 ' if self.timed else ''
            data = f'{timing}uptime_ms={self.uptime} display_submissions={self.submissions} input_events={self.input_events}'
        elif command == 'HARNESS CAPS':
            data = 'capture=supported input=supported buttons=0,1,2,3, boot=9'
        elif words[:2] == ['HARNESS', 'OPEN']:
            data = f'nonce={words[2]} boot=9 session=5'
        elif words[2:] == ['CLOSE']:
            data = 'boot=9 session=0'
        elif words[2:4] == ['INPUT', 'ADD']:
            return {'status': 'ACCEPTED', 'data': common}
        elif command == 'ACTIVITY':
            self.active = self.uptime
            data = common
        elif words[2:4] == ['INPUT', 'RUN']:
            # Like the shell's wake gate: a press on a dimmed screen only wakes it.
            if self.dimmed():
                self.active, self.wakes = self.uptime, self.wakes + 1
                return {'status': 'ACCEPTED', 'data': common}
            self.active = self.uptime
            self.presses += 1
            self.last = self.latencies.pop(0)
            self.input_events += self.presses in self.touched_at
            return {'status': 'ACCEPTED', 'data': common}
        elif words[2:4] == ['INPUT', 'STATUS']:
            data = f'{common} state=completed accepted=1 delivered=1 losses=0 held=false'
        else:
            data = common
        return {'status': 'OK', 'data': data}


class LatencyTests(unittest.TestCase):
    def run_tool(self, fake, **options):
        with tempfile.TemporaryDirectory() as directory:
            runner = Runner(fake, Path(directory))
            try:
                return measure(runner, 0, **options)
            finally:
                runner.trace.close()

    def test_reports_each_press_and_the_idle_frame_rate(self):
        # The untimed setup press also draws a frame.
        fake = FakeDevice([5, 20, 14, 30, 16])
        result = self.run_tool(fake, count=4, setup=[1], idle_s=5, seed=1)
        self.assertEqual(result['press_frame_ms'], [20, 14, 30, 16])
        self.assertEqual((result['median_ms'], result['max_ms']), (18, 30))
        self.assertEqual(result['idle_frames_per_s'], 2.0)
        self.assertEqual(result['idle_frame_ms'], 13)
        adds = [c for c in fake.commands if ' INPUT ADD ' in c]
        # One untimed setup press at once, then timed presses at random phases past the debounce.
        self.assertEqual(adds[0].split()[-3:], ['0', 'BUTTON', '1'])
        self.assertTrue(all(400 <= int(c.split()[-3]) < 900 for c in adds[1:]))
        self.assertEqual(fake.commands[-1], 'HARNESS 5 CLOSE')

    def test_wakes_a_screen_dimmed_by_the_idle_window_before_timing(self):
        # A 5 s dim timeout and a 5 s idle window: the screen is dim when timing starts.
        fake = FakeDevice([5, 20, 14], dim_ms=5000)
        result = self.run_tool(fake, count=2, setup=[1], idle_s=5, seed=1)
        self.assertEqual(result['press_frame_ms'], [20, 14])
        self.assertEqual(fake.wakes, 0, 'no timed press was spent waking the screen')
        self.assertGreater(fake.commands.count('ACTIVITY'), 3)

    def test_sends_nothing_from_a_press_until_it_is_due_and_drawn(self):
        # Terminal traffic would keep the console at its busy pace.
        fake = FakeDevice([5, 20, 14])
        self.run_tool(fake, count=2, setup=[1], idle_s=5, seed=1)
        runs = [i for i, command in enumerate(fake.commands) if ' INPUT RUN ' in command][1:]
        dues = [int(command.split()[-3]) for command in fake.commands if ' INPUT ADD ' in command][1:]
        self.assertEqual(len(runs), 2)
        for run, due in zip(runs, dues, strict=True):
            self.assertGreaterEqual(fake.sent[run + 1] - fake.sent[run], due + 250, fake.commands[run + 1])

    def test_fails_a_run_a_hand_touched_during_the_timed_presses(self):
        # VANA ignores a synthetic press within 350 ms of a physical one, so its sample waits for an unrelated frame.
        result = self.run_tool(FakeDevice([5, 20, 274, 16], touched_at={2}), count=3, setup=[1], idle_s=5, seed=1)
        self.assertEqual(result['status'], 'fail')
        self.assertEqual(result['physical_inputs'], 1)
        self.assertEqual(result['press_frame_ms'], [20, 274, 16])

    def test_a_touch_before_the_timed_presses_does_not_fail_the_run(self):
        result = self.run_tool(FakeDevice([5, 20, 14], touched_at={1}), count=2, setup=[1], idle_s=5, seed=1)
        self.assertEqual((result['status'], result['physical_inputs']), ('pass', 0))

    def test_refuses_firmware_without_press_timing(self):
        with self.assertRaisesRegex(RuntimeError, 'press_frame_ms and display_ms'):
            self.run_tool(FakeDevice([], timed=False), count=1)


if __name__ == '__main__':
    unittest.main()
