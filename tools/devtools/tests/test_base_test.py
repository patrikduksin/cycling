import unittest
from unittest.mock import Mock, patch

from cycling_devtools.harness.base import (
    apply,
    check_overlong,
    crash_record,
    expect,
    idle_frames_allowed,
    power_cycle,
    preserved,
    settle,
    transport_recovery,
    wifi_failures,
)


class Connection:
    def __init__(self, status='OK'):
        self.request_id = 5
        self.status = status
        self.commands = []

    def terminal_command(self, command):
        self.commands.append(command)
        if command == 'SAVE':
            raise TimeoutError('completion unknown')
        return dict(status=self.status, data='stats=(2, 3, 4, 0)')


class BaseTests(unittest.TestCase):
    def test_crash_record_waits_out_startup_busy(self):
        replies = [
            dict(status='BUSY', data='starting companion=probing'),
            dict(status='BUSY', data='starting companion=probing'),
            dict(status='OK', data='kind=controlled this_boot=true message=controlled crash test'),
        ]
        connection = Mock(terminal_command=Mock(side_effect=replies))
        with patch('cycling_devtools.harness.base.time.sleep') as sleep:
            record = crash_record(connection)
        self.assertEqual(record, dict(kind='controlled', this_boot='true', message='controlled crash test'))
        self.assertEqual(connection.terminal_command.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_crash_record_gives_up_when_startup_stopped(self):
        connection = Mock(terminal_command=Mock(return_value=dict(status='BUSY', data='starting companion=charging')))
        with patch('cycling_devtools.harness.base.time.sleep'), self.assertRaises(AssertionError):
            crash_record(connection)
        self.assertEqual(connection.terminal_command.call_count, 1)

    def test_crash_record_stops_retrying_after_the_deadline(self):
        connection = Mock(terminal_command=Mock(return_value=dict(status='BUSY', data='starting companion=probing')))
        with (
            patch('cycling_devtools.harness.base.time.sleep'),
            patch('cycling_devtools.harness.base.time.monotonic', side_effect=[0, 1, 31]),
            self.assertRaises(AssertionError),
        ):
            crash_record(connection)
        self.assertEqual(connection.terminal_command.call_count, 2)

    def test_base_firmware_draws_nothing_while_idle(self):
        self.assertEqual(idle_frames_allowed(dict(cycling='false'), 60_000), 0)

    def test_vana_may_draw_twice_a_second_while_idle(self):
        self.assertEqual(idle_frames_allowed(dict(cycling='true'), 5_400), 12)
        self.assertEqual(idle_frames_allowed(dict(cycling='true'), 10_900), 23)
        self.assertLess(idle_frames_allowed(dict(cycling='true'), 10_900), 26)

    def test_ambiguous_save_is_not_replayed(self):
        connection = Connection()
        settings = dict(brightness='50', dim_timeout='30', dim_brightness='20', timezone='-180')
        with self.assertRaises(TimeoutError):
            apply(connection, settings)
        self.assertEqual(connection.commands.count('SAVE'), 1)

    def test_invalid_parser_reply_uses_reserved_id_zero(self):
        connection = Connection('INVALID')
        expect(connection, 'SAVE extra', 'INVALID')
        self.assertEqual(connection.request_id, -1)  # Client increments before transmission.

    def test_restore_checks_every_persisted_preference(self):
        original = dict(brightness='50', dim_timeout='30', dim_brightness='20', timezone='-180')
        for key in original:
            with self.assertRaises(AssertionError):
                preserved(original, dict(original, **{key: 'wrong'}))

    def test_wifi_failure_counter_is_not_association_count(self):
        self.assertEqual(wifi_failures(Connection()), 4)


class ScriptedConnection:
    """Replies per command, in order; the last reply of a command repeats."""

    def __init__(self, replies):
        self.replies = {command: list(answers) for command, answers in replies.items()}
        self.commands = []
        self.request_id = 1

    def terminal_command(self, command):
        self.commands.append(command)
        answers = self.replies[command]
        status, data = answers.pop(0) if len(answers) > 1 else answers[0]
        return dict(status=status, data=data, ms=1)

    def __exit__(self, *unused):
        pass


IDLE = 'status=Status { state: Idle, ready: true }'


class UsbStandby:
    """A USB-powered C606 after POWER SHUTDOWN, as measured on aca9b77.

    It resets in software into charging standby and stays there. POWER WAKE
    starts recovery, and a few STATUS polls later the companion cuts power:
    the session drops and the C606 boots with a power-on reset.
    """

    PHASES = {
        'standby': {
            'STATUS': ('OK', 'uptime_ms=10000 reset=software crash=none'),
            'POWER': ('OK', 'status=Status { state: Charging, ready: false }'),
            'COMPANION': ('OK', 'startup=charging'),
            'CRASH': ('BUSY', 'power transition'),
        },
        'recovering': {
            'STATUS': ('OK', 'uptime_ms=12000 reset=software crash=none'),
            'POWER': ('OK', 'status=Status { state: Recovering, ready: false }'),
            'COMPANION': ('OK', 'startup=charging'),
            'CRASH': ('BUSY', 'power transition'),
        },
        'on': {
            'STATUS': ('OK', 'uptime_ms=3000 reset=power crash=none'),
            'POWER': ('OK', IDLE),
            'COMPANION': ('OK', 'startup=ready'),
            'CRASH': ('OK', 'kind=controlled this_boot=false source=flash message=controlled crash test'),
        },
    }

    def __init__(self, polls_until_reset=3):
        self.phase = 'standby'
        self.boot = 0
        self.polls_until_reset = polls_until_reset
        self.commands = []

    def connect(self):
        return UsbStandbySession(self, self.boot)

    def answer(self, command):
        self.commands.append(command)
        if command == 'POWER WAKE':
            if self.phase != 'standby':
                return dict(status='UNAVAILABLE', data='', ms=1)
            self.phase = 'recovering'
            return dict(status='ACCEPTED', data='', ms=1)
        if self.phase == 'recovering' and command == 'STATUS':
            self.polls_until_reset -= 1
            if self.polls_until_reset == 0:
                self.phase = 'on'
                self.boot += 1
                raise RuntimeError('device rebooted during command')
        status, data = self.PHASES[self.phase][command]
        return dict(status=status, data=data, ms=1)


class UsbStandbySession:
    def __init__(self, device, boot):
        self.device = device
        self.boot = boot
        self.request_id = 1

    def terminal_command(self, command):
        if self.boot != self.device.boot:
            raise RuntimeError('device disconnected; command outcome may be uncertain')
        return self.device.answer(command)

    def __exit__(self, *unused):
        pass


class PowerCycleTests(unittest.TestCase):
    def cycle(self, before, after):
        with patch('cycling_devtools.harness.base.reopen', side_effect=[before, after]), patch('time.sleep'):
            return power_cycle('port', None)

    def test_switches_off_once_and_wakes_charging_standby_before_reading_flash(self):
        before = ScriptedConnection({'POWER SHUTDOWN': [('UNAVAILABLE', ''), ('ACCEPTED', '')]})
        idle = 'status=Status { state: Idle, ready: true }'
        after = ScriptedConnection(
            {
                'STATUS': [('OK', 'reset=power crash=none')],
                'COMPANION': [
                    ('OK', 'startup=probing'),
                    ('OK', 'startup=charging'),
                    ('OK', 'startup=charging'),
                    ('OK', 'startup=ready'),
                ],
                'POWER': [
                    ('OK', idle),
                    ('OK', 'status=Status { state: Charging, ready: false }'),
                    ('OK', 'status=Status { state: Charging, ready: false }'),
                    ('OK', idle),
                ],
                'POWER WAKE': [('UNAVAILABLE', ''), ('ACCEPTED', '')],
            }
        )
        self.assertIs(self.cycle(before, after), after)
        self.assertEqual(before.commands, ['POWER SHUTDOWN', 'POWER SHUTDOWN'])
        self.assertEqual(after.commands.count('POWER WAKE'), 2)
        self.assertEqual(after.commands[-2:], ['COMPANION', 'POWER'])

    def test_wakes_usb_charging_standby_before_judging_the_reset(self):
        before = ScriptedConnection({'POWER SHUTDOWN': [('UNAVAILABLE', ''), ('ACCEPTED', '')]})
        device = UsbStandby()
        sessions = iter([before])
        with (
            patch(
                'cycling_devtools.harness.base.reopen',
                side_effect=lambda *unused: next(sessions, None) or device.connect(),
            ),
            patch('time.sleep'),
        ):
            connection = power_cycle('port', None)
            record = crash_record(connection)
        self.assertEqual(device.commands.count('POWER WAKE'), 1)
        self.assertEqual(device.phase, 'on')
        self.assertEqual(record['source'], 'flash')

    def test_an_early_idle_before_the_companion_starts_is_not_settled(self):
        connection = ScriptedConnection(
            {
                'COMPANION': [('OK', 'startup=waiting'), ('OK', 'startup=ready')],
                'POWER': [('OK', 'status=Status { state: Idle, ready: true }')],
            }
        )
        with patch('time.sleep'):
            settle(connection)
        self.assertEqual(connection.commands, ['COMPANION', 'POWER', 'COMPANION', 'POWER'])

    def test_a_reset_that_kept_rtc_memory_fails(self):
        before = ScriptedConnection({'POWER SHUTDOWN': [('ACCEPTED', '')]})
        after = ScriptedConnection({'STATUS': [('OK', 'reset=software crash=none')]})
        with self.assertRaisesRegex(AssertionError, 'power-on reset'):
            self.cycle(before, after)

    def test_a_refused_shutdown_never_waits_for_power(self):
        before = ScriptedConnection({'POWER SHUTDOWN': [('BUSY', '')]})
        with self.assertRaisesRegex(AssertionError, 'BUSY'):
            self.cycle(before, None)


class FramingTests(unittest.TestCase):
    def test_probe_exceeds_real_256_byte_limit_and_preserves_suffix_test(self):
        class FramingConnection(Connection):
            def terminal_command(self, command):
                self.commands.append(command)
                # The production receiver advertises 256, not the old probe's 128.
                frame = f'CMD 0 {command}'.encode('ascii')
                return dict(
                    status='INVALID', data='line exceeds 256 bytes' if len(frame) > 256 else 'malformed command'
                )

        connection = FramingConnection()
        check_overlong(connection, {'max_line': '256'})
        self.assertEqual(len(connection.commands), 1)
        self.assertGreater(len(connection.commands[0].encode('ascii')), 256)
        self.assertTrue(connection.commands[0].endswith(' RESTART'))
        self.assertNotIn('\n', connection.commands[0])
        self.assertEqual(connection.request_id, -1)

    def test_invalid_or_unbounded_advertisement_never_sends_a_probe(self):
        for value in (None, '', 'bad', '-1', '0', '15', '4097', '1000000000', 256, True):
            connection = Connection()
            with self.subTest(value=value), self.assertRaises(ValueError):
                check_overlong(connection, {'max_line': value})
            self.assertEqual(connection.commands, [])

    def test_generic_invalid_and_wrong_limit_replies_do_not_pass(self):
        from unittest.mock import Mock

        for message in ('malformed command', 'line exceeds 128 bytes'):
            connection = Mock()
            connection.terminal_command.return_value = dict(status='INVALID', data=message)
            with self.subTest(message=message), self.assertRaises(AssertionError):
                check_overlong(connection, {'max_line': '256'})
            connection.terminal_command.assert_called_once()


class StallConnection:
    def __init__(self, timeout=False):
        self.commands = []
        self.timeout = timeout

    def terminal_command(self, command, timeout=8):
        self.commands.append((command, timeout))
        if self.timeout:
            raise TimeoutError('stall completion unknown')
        return dict(status='ACCEPTED', ms=100, data='')


class TransportRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.calls = 0

    def read(self, connection, command):
        self.calls += 1
        now = self.calls * 500
        stalled = bool(connection.commands)
        if command == 'POSITION':
            return dict(
                ms=now,
                transport='receiving',
                fix='no_fix',
                bytes=str(now),
                valid=str(self.calls),
                checksum_errors='0',
                parse_errors='0',
                dma_losses=str(int(stalled)),
                line_overflows='0',
                uart_errors='0',
            )
        if command == 'INPUT':
            return dict(
                ms=now,
                companion_valid=str(self.calls),
                bad_crc='0',
                uart_errors=str(int(stalled)),
                touch_errors='0',
                input_lost='0',
            )
        if command == 'SETTINGS':
            return dict(brightness='50', dim_timeout='30', dim_brightness='20', timezone='-180')
        return dict(ms=now, storage_ops='0')

    def test_stalls_once_and_records_loss_without_requiring_an_indoor_fix(self):
        connection = StallConnection()
        report = {}
        with (
            patch('cycling_devtools.harness.base.read', side_effect=self.read),
            patch('cycling_devtools.harness.base.time.sleep'),
        ):
            transport_recovery(connection, dict(cycling='false', harness='true'), report)
        self.assertEqual(connection.commands, [('TEST 20', 12)])
        self.assertTrue(report['recovered'])
        self.assertEqual(report['position_delta']['dma_losses'], 1)
        self.assertEqual(report['input_delta']['uart_errors'], 1)
        self.assertEqual(report['settled_position']['fix'], 'no_fix')

    def test_timeout_never_replays_stall_and_preserves_baseline(self):
        connection = StallConnection(timeout=True)
        report = {}
        with patch('cycling_devtools.harness.base.read', side_effect=self.read), self.assertRaises(TimeoutError):
            transport_recovery(connection, dict(cycling='false', harness='true'), report)
        self.assertEqual(connection.commands, [('TEST 20', 12)])
        self.assertIn('before_position', report)
        self.assertFalse(report['recovered'])

    def test_sdk_build_is_rejected_before_any_command(self):
        connection = StallConnection()
        with self.assertRaises(ValueError):
            transport_recovery(connection, dict(cycling='true', harness='true'), {})
        self.assertEqual(connection.commands, [])

    def test_persistent_failure_is_not_treated_as_recovery(self):
        def failed(connection, command):
            result = self.read(connection, command)
            if command == 'POSITION':
                result['transport'] = 'failed'
            return result

        report = {}
        with (
            patch('cycling_devtools.harness.base.read', side_effect=failed),
            patch('cycling_devtools.harness.base.time.monotonic', side_effect=[0, 16]),
            self.assertRaises(TimeoutError),
        ):
            transport_recovery(StallConnection(), dict(cycling='false', harness='true'), report)
        self.assertFalse(report['recovered'])


if __name__ == '__main__':
    unittest.main()
