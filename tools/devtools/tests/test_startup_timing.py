import unittest

import pytest

import cycling_devtools.harness.runner as runner_module
import cycling_devtools.harness.transport as transport_module
from cycling_devtools.harness import startup_timing
from cycling_devtools.harness.startup_timing import parse, poll, table

REPLY = (
    'first_frame_ms=3310 home_ms=3710 companion_ms=3310 storage_ms=3310 gnss_fix_ms=- '
    'sensors=120:9100,11:9400 ride_press_ms=20000 recording_ms=20050 '
    'press_frame_last_ms=12 press_frame_max_ms=40 '
)


class StartupTimingTests(unittest.TestCase):
    def test_parse_reads_milestones_sensors_and_missing_values(self):
        fields = parse(REPLY)
        self.assertEqual(fields['first_frame_ms'], 3310)
        self.assertIsNone(fields['gnss_fix_ms'])
        self.assertEqual(fields['sensors'], {120: 9100, 11: 9400})
        self.assertEqual(parse('sensors=-')['sensors'], {})

    def test_table_reports_steps_from_their_base_milestone(self):
        report = table(parse(REPLY), 'test')
        self.assertIn('| Home accepts input | 3710 | 400 |', report)
        self.assertIn('| Recording confirmed | 20050 | 50 |', report)
        self.assertIn('| GNSS fix | — | — |', report)
        self.assertIn('| ANT heart live data | 9100 | — |', report)
        self.assertIn('| Press to next frame, worst ride press | — | 40 |', report)
        # Sensors are listed in the order they came up.
        self.assertLess(report.index('ANT heart'), report.index('ANT power'))

    def test_poll_treats_busy_startup_as_pending_until_the_milestones_arrive(self):
        replies = [
            {'status': 'BUSY', 'data': 'power transition'},
            {'status': 'BUSY', 'data': 'power transition'},
            {'status': 'OK', 'data': 'first_frame_ms=- home_ms=- sensors=-'},
            {'status': 'OK', 'data': REPLY},
        ]
        runner = FakeRunner(replies)
        fields = poll(runner, ('first_frame_ms', 'home_ms'), 60, sleep=lambda _: None)
        self.assertEqual(fields['home_ms'], 3710)
        self.assertEqual(runner.transport.sent, 4)

    def test_poll_fails_on_errors_and_on_busy_for_the_whole_wait(self):
        with self.assertRaises(RuntimeError):
            poll(FakeRunner([{'status': 'INVALID', 'data': ''}]), (), 60, sleep=lambda _: None)
        with self.assertRaises(TimeoutError):
            poll(FakeRunner([{'status': 'BUSY', 'data': ''}] * 3), ('home_ms',), 0, sleep=lambda _: None)


class FakeTransport:
    def __init__(self, replies):
        self.replies, self.sent = list(replies), 0

    def terminal_command(self, command, timeout=8):
        assert command == 'RIDE TIMING'
        self.sent += 1
        return self.replies.pop(0) if len(self.replies) > 1 else self.replies[0]


class FakeRunner:
    def __init__(self, replies):
        self.transport = FakeTransport(replies)


if __name__ == '__main__':
    unittest.main()


def test_an_open_ride_is_never_driven_or_restarted():
    for info in ({'recording': 'true'}, {}):
        with pytest.raises(RuntimeError, match='ride is open'):
            startup_timing.require_idle(info)
    startup_timing.require_idle({'recording': 'false'})


def test_ride_refuses_before_any_press_when_a_ride_is_recording(monkeypatch, tmp_path):
    pressed = []

    class Runner:
        def __init__(self, transport, directory):
            self.trace = type('Trace', (), {'close': lambda self: None})()

        def command(self, command):
            return {'recording': 'true'} if command == 'INFO' else {}

    class Real:
        def __init__(self, port, log):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def terminal_command(self, command):
            pressed.append(command)
            return {'status': 'OK', 'data': 'recording=true'}

    monkeypatch.setattr(runner_module, 'Runner', Runner)
    monkeypatch.setattr(transport_module, 'Real', Real)
    monkeypatch.setattr(startup_timing, 'wait_for_port', lambda parser, port, timeout: 'port')
    monkeypatch.setattr(startup_timing, 'poll', lambda runner, keys, timeout: {})
    monkeypatch.setattr(startup_timing, 'ride', lambda runner: pressed.append('ride'))
    args = type('Args', (), {'restart': False, 'ride': True, 'gnss': False, 'seconds': 1, 'port': None})()
    with pytest.raises(RuntimeError, match='ride is open'):
        startup_timing.device(None, args, tmp_path)
    assert 'ride' not in pressed
    # A restart is refused before RESTART is sent.
    args.restart, args.ride = True, False
    with pytest.raises(RuntimeError, match='ride is open'):
        startup_timing.device(None, args, tmp_path)
    assert 'RESTART' not in pressed


class Vana:
    """A tiny model of VANA's pages, buttons and recorder behind `RIDE STATUS`."""

    def __init__(self, page='home', state='ready', cursor=0, ignore_save=False):
        self.page, self.state, self.cursor = page, state, cursor
        self.rides, self.armed, self.ignore_save = 0, False, ignore_save
        self.edited = False
        self.presses = []
        self.session = None

    def command(self, command):
        if command == 'RIDE STATUS':
            return {
                'state': self.state,
                'rides': str(self.rides),
                'page': self.page,
                'home_cursor': str(self.cursor),
            }
        return {'recording': 'false'}

    def open_session(self):
        self.session = '1'

    def operation(self, command):
        return {}

    def delay(self, seconds):
        pass

    def inject(self, step):
        button = {0: 'top-left', 1: 'bottom-left', 2: 'bottom-right'}[step['button']]
        self.presses.append(button)
        if self.page == 'settings':
            self.edited = True
        elif self.page == 'home':
            if button == 'bottom-left':
                self.cursor = (self.cursor + 1) % 3
            elif button == 'bottom-right':
                self.page = ('ride', 'sensors', 'settings')[self.cursor]
        elif self.page == 'ride':
            if button == 'bottom-right':
                self.state = {'recording': 'paused', 'paused': 'recording'}.get(self.state, 'recording')
            elif button == 'top-left' and self.state == 'paused':
                if self.armed and not self.ignore_save:
                    self.state, self.rides = 'saved', self.rides + 1
                self.armed = True


def test_ride_refuses_away_from_home_without_pressing():
    for page, state in (('settings', 'ready'), ('ride', 'saved'), ('power', 'ready'), ('home', 'recording')):
        vana = Vana(page, state)
        with pytest.raises(RuntimeError, match='starts from Home'):
            startup_timing.ride(vana)
        assert vana.presses == [], (page, state)
        assert not vana.edited


def test_ride_walks_home_to_train_and_ends_with_the_ride_saved():
    vana = Vana(cursor=2)
    startup_timing.ride(vana)
    assert vana.presses[:2] == ['bottom-left', 'bottom-right']
    assert (vana.state, vana.rides, vana.edited) == ('saved', 1, False)
    assert vana.session is None


def test_ride_reports_a_ride_that_did_not_save():
    vana = Vana(ignore_save=True)
    with pytest.raises(RuntimeError, match='state=saved'):
        startup_timing.ride(vana)
    assert vana.session is None


def test_ble_cadence_first_data_has_its_own_row():
    report = table(parse('first_frame_ms=- sensors=1:5200,0:5100'), 'C606')
    assert '| BLE cadence live data | 5200 |' in report
    assert '| BLE heart live data | 5100 |' in report


def test_a_failed_storage_scan_and_dropped_sensors_are_shown():
    fields = parse('storage_ms=- storage_state=error sensors=- sensors_dropped=2 ')
    assert fields['storage_state'] == 'error'
    report = table(fields, 'C606')
    assert '| Ride storage ready | — | — |' in report
    assert 'scan **failed** (state=error)' in report
    assert '| Sensor kinds not recorded (over capacity) | 2 |' in report


def test_summary_reports_median_and_p90_of_each_milestone_and_step():
    runs = [
        parse(
            f'first_frame_ms={3000 + i * 10} home_ms={3400 + i * 10} sensors=120:{9000 + i} '
            f'ride_press_ms=20000 recording_ms={20030 + i} press_frame_last_ms={10 + i} '
            f'press_frame_max_ms={40 + i} storage_state=ready'
        )
        for i in range(10)
    ]
    report = startup_timing.summarize(runs)
    assert '(n=10)' in report
    # Median of 3000..3090 is 3045; nearest-rank p90 is the 9th value, 3080.
    assert '| Power-on to first frame (backlight on) | 3045 | 3080 | — | — | 10 |' in report
    assert '| Home accepts input | 3445 | 3480 | 400 | 400 | 10 |' in report
    assert '| Recording confirmed | 20034 | 20038 | 34 | 38 | 10 |' in report
    assert '| Press to next frame, worst ride press | — | — | 44 | 48 | 10 |' in report
    assert '| ANT heart live data | 9004 | 9008 | — | — | 10 |' in report
