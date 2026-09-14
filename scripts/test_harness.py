import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import zlib

from harness import Runner, decode_chunk, events_for, png_rgb565, preflight, execute
from harness_transport import Real


class CleanupTransport:
    """Records commands and separates lost replies from whether a mutation acted."""
    virtual = True
    def __init__(self, close_timeout=None, restore_timeout=None, apply_before_timeout=False):
        self.original = {'brightness': '61', 'timezone': '-180', 'dim_timeout': '33', 'dim_brightness': '22'}
        self.settings = dict(self.original)
        self.commands, self.closed, self.request_id = [], False, 1
        self.close_timeout, self.restore_timeout = close_timeout, restore_timeout
        self.apply_before_timeout = apply_before_timeout
        self.status_reads, self.now = 0, 100.0
    def __enter__(self):
        return self
    def __exit__(self, *_):
        self.closed = True
    def terminal_command(self, command, **_):
        self.commands.append(command)
        self.request_id += 1
        if command == 'INFO':
            data = 'cycling=false recording=false'
        elif command == 'HARNESS CAPS':
            data = ('input=supported capture=supported boot=1 '
                    'ordinary=INFO,STATUS,SETTINGS,BRIGHTNESS,TIMEZONE,IDLE')
        elif command.startswith('HARNESS OPEN '):
            data = f'nonce={command.split()[2]} boot=1 session=7'
        elif command == 'HARNESS 7 CLOSE':
            if self.close_timeout is not None:
                self.now += self.close_timeout
                raise TimeoutError('CLOSE reply lost')
            data = 'boot=1 session=0'
        elif command == 'HARNESS 7 PING':
            raise TimeoutError('session unavailable after uncertain CLOSE')
        elif command == 'STATUS':
            self.status_reads += 1
            if self.status_reads > 1:
                raise KeyboardInterrupt()
            data = 'input_events=0'
        elif command == 'SETTINGS':
            data = ' '.join(f'{key}={value}' for key, value in self.settings.items())
        else:
            if command == self.restore_timeout and not self.apply_before_timeout:
                raise TimeoutError('preference mutation reply lost')
            words = command.split()
            if words[0] == 'BRIGHTNESS':
                self.settings['brightness'] = words[1]
            elif words[0] == 'TIMEZONE':
                self.settings['timezone'] = words[1]
            elif words[0] == 'IDLE':
                self.settings['dim_timeout'], self.settings['dim_brightness'] = words[1:]
            else:
                raise AssertionError(command)
            if command == self.restore_timeout:
                raise TimeoutError('preference mutation reply lost')
            data = ''
        return {'status': 'OK', 'data': data}


class HarnessTests(unittest.TestCase):
    def test_preflight_rejects_persistence_and_malformed_sequences_before_transport(self):
        for command in ('SAVE', 'RIDE START', 'EXPORT CLEAR', 'BRIGHTNESS 10\nRESTART', 'RESTART'):
            with self.assertRaises(ValueError):
                preflight({'steps': [{'op': 'command', 'command': command}]})
        for events in ([{'at_ms': 0, 'event': 'DOWN 1 1'}], [{'at_ms': 0, 'event': 'UP'}],
                       [{'at_ms': 30001, 'event': 'BUTTON 0'}]):
            with self.assertRaises(ValueError):
                events_for({'events': events})
        with self.assertRaises(ValueError):
            events_for({'gesture': 'button-hold'})

    def test_typed_peripheral_allowlist_excludes_sustained_sound_and_writes(self):
        for step in [dict(op='sound',action='play',pattern=18),dict(op='sound',action='play',pattern=24),dict(op='mmc',action='write',sector=0),dict(op='mmc',action='read',sector=0,offset=511,length=2),dict(op='gnss',action='pause',duration_ms=60000)]:
            with self.assertRaises(ValueError): preflight({'steps':[step]})
        for step in [dict(op='sound',action='play',pattern=22),dict(op='sound',action='stop'),dict(op='mmc',action='read',sector=0),dict(op='gnss',action='pause',duration_ms=4000)]:
            preflight({'steps':[step]})

    def test_failed_media_cleanup_does_not_skip_radio_or_preferences(self):
        class FailedMedia(CleanupTransport):
            def terminal_command(self, command, **kwargs):
                if command == 'MMC': return {'status':'OK','data':'clock_hz=400000 sectors=8'}
                if command.startswith('MMC '): return {'status':'FAILED','data':''}
                return super().terminal_command(command, **kwargs)
        fake=FailedMedia()
        scenario={'connectivity_restore':{'wifi':{'profile':None,'connected':False}},'steps':[
            dict(op='command',command='BRIGHTNESS 20'),dict(op='wifi',action='forget'),dict(op='mmc',action='read',sector=0)]}
        args=SimpleNamespace(run_id=None,agent_tool_calls=None,backend='virtual',state_dir=None,sdk=False,no_harness=False)
        actions=[]
        with tempfile.TemporaryDirectory() as directory:
            output=Path(directory)
            with patch('harness.Virtual',return_value=fake),patch.object(Runner,'connectivity',side_effect=lambda radio,step: actions.append((radio,step['action']))),patch('sys.stdout',io.StringIO()):
                code=execute(args,scenario,output)
            report=json.loads((output/'report.json').read_text())
        self.assertEqual(code,2)
        self.assertIn('uncertain',report['cleanup']['mmc'])
        self.assertEqual(actions,[('wifi','forget'),('wifi','forget')])
        self.assertEqual(report['cleanup']['preferences'],'verified')
        self.assertEqual(fake.settings,fake.original)

    def test_connectivity_terminal_mismatch_fails_without_delay(self):
        for expected, actual, code in [('completed', 'error', 'peer_not_found'),
                                       ('error', 'completed', 'none'),
                                       ('completed', 'error', 'private-secret')]:
            with self.subTest(expected=expected, actual=actual, code=code):
                runner = object.__new__(Runner)
                replies = [{'sequence': '4', 'operation': 'completed'}, {},
                           {'sequence': '5', 'operation': actual, 'error': code}]
                with patch.object(runner, 'command', side_effect=replies), \
                     patch.object(runner, 'delay') as delay:
                    with self.assertRaises(RuntimeError) as caught:
                        runner.connectivity('ble', {'action': 'connect', 'completion': expected})
                delay.assert_not_called()
                self.assertIn('BLE operation ' + actual, str(caught.exception))
                self.assertIn('error=' + ('unknown' if code == 'private-secret' else code), str(caught.exception))
                self.assertNotIn('private-secret', str(caught.exception))

    def test_connectivity_ignores_previous_sequence_terminal_state(self):
        runner = object.__new__(Runner)
        replies = [{'sequence': '4', 'operation': 'completed'}, {},
                   {'sequence': '4', 'operation': 'error', 'error': 'peer_not_found'},
                   {'sequence': '5', 'operation': 'pending'},
                   {'sequence': '5', 'operation': 'completed'}]
        with patch.object(runner, 'command', side_effect=replies), \
             patch.object(runner, 'delay') as delay:
            result = runner.connectivity('ble', {'action': 'connect'})
        self.assertEqual(result['operation'], 'completed')
        self.assertEqual(delay.call_count, 2)

    def test_connectivity_failure_still_restores_radio_and_preferences(self):
        class FailedConnect(CleanupTransport):
            sequence, radio_operation, radio_error = 0, 'completed', 'none'
            def terminal_command(self, command, **kwargs):
                if command == 'BLE':
                    return {'status': 'OK', 'data': f'sequence={self.sequence} operation={self.radio_operation} error={self.radio_error}'}
                if command in ('BLE CONNECT', 'BLE FORGET'):
                    self.commands.append(command)
                    self.sequence += 1
                    self.radio_operation = 'error' if command == 'BLE CONNECT' else 'completed'
                    self.radio_error = 'peer_not_found' if command == 'BLE CONNECT' else 'none'
                    return {'status': 'ACCEPTED', 'data': ''}
                return super().terminal_command(command, **kwargs)
        fake = FailedConnect()
        scenario = {'connectivity_restore': {'ble': {'mode': 'none', 'connected': False}},
                    'steps': [dict(op='command', command='BRIGHTNESS 20'), dict(op='ble', action='connect')]}
        args = SimpleNamespace(run_id=None, agent_tool_calls=None, backend='virtual', state_dir=None,
                               sdk=True, no_harness=False)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with patch('harness.Virtual', return_value=fake), patch.object(Runner, 'delay') as delay, \
                 patch('sys.stdout', io.StringIO()):
                code = execute(args, scenario, output)
            report = json.loads((output / 'report.json').read_text())
        self.assertEqual(code, 1)
        delay.assert_not_called()
        self.assertIn('BLE FORGET', fake.commands)
        self.assertIn('completion verified', report['cleanup']['ble'])
        self.assertEqual(report['cleanup']['preferences'], 'verified')
        self.assertEqual(fake.settings, fake.original)

    def test_png_exact_geometry_and_pixels(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'frame.png'
            png_rgb565(path, struct.pack('<4H', 0xf800, 0x7e0, 0x1f, 0xffff), 2, 2)
            encoded = path.read_bytes()
            pos, payload = 8, b''
            while pos < len(encoded):
                size = struct.unpack_from('>I', encoded, pos)[0]
                kind, data = encoded[pos + 4:pos + 8], encoded[pos + 8:pos + 8 + size]
                self.assertEqual(zlib.crc32(kind + data), struct.unpack_from('>I', encoded, pos + 8 + size)[0])
                if kind == b'IDAT':
                    payload += data
                pos += size + 12
            self.assertEqual(zlib.decompress(payload), b'\0\xff\0\0\0\xff\0\0\0\0\xff\xff\xff\xff')

    def test_chunk_decoding_rejects_corruption_and_bounds(self):
        payload = b'\0\xf8' * 2048
        chunk = {'frame': '1', 'offset': '0', 'length': '4096', 'encoding': 'rle565',
                 'hex': struct.pack('<HH', 2048, 0xf800).hex(), 'crc32': f'{zlib.crc32(payload):08x}'}
        self.assertEqual(decode_chunk(chunk, 1, 0, 4096), payload)
        for replacement in ({'frame': '2'}, {'offset': '2'}, {'crc32': '00000000'},
                            {'hex': struct.pack('<HH', 2049, 0xf800).hex()}, {'hex': '00000000'}):
            with self.assertRaises(ValueError):
                decode_chunk(chunk | replacement, 1, 0, 4096)

    def run_cleanup_case(self, fake, captures=()):
        scenario = {'steps': [
            {'op': 'command', 'command': 'BRIGHTNESS 50'},
            {'op': 'virtual-command', 'command': 'TIMEZONE 0'},
            {'op': 'virtual-command', 'command': 'IDLE 30 10'},
            {'op': 'command', 'command': 'STATUS'},
        ], 'captures': [{'kind': capture.kind} for capture in captures]}
        args = SimpleNamespace(run_id=None, agent_tool_calls=None, backend='virtual',
                               state_dir=None, sdk=False, no_harness=False)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with patch('harness.Virtual', return_value=fake), \
                 patch('harness.Capture', side_effect=captures), \
                 patch('harness.time.monotonic', side_effect=lambda: fake.now), \
                 patch('sys.stdout', io.StringIO()):
                code = execute(args, scenario, output)
            report = json.loads((output / 'report.json').read_text())
        self.assertTrue(fake.closed)
        self.assertNotIn('SAVE', fake.commands)
        return code, report

    def test_interruption_closes_session_before_restoring_all_unsaved_preferences(self):
        fake = CleanupTransport()
        code, report = self.run_cleanup_case(fake)
        self.assertEqual(code, 1)
        self.assertEqual(report['status'], 'fail')
        self.assertEqual(report['cleanup']['input_capture'], 'closed')
        self.assertEqual(report['cleanup']['preferences'], 'verified')
        self.assertEqual(fake.settings, fake.original)
        close = fake.commands.index('HARNESS 7 CLOSE')
        self.assertEqual(fake.commands[close + 1:], [
            'SETTINGS', 'BRIGHTNESS 61', 'TIMEZONE -180', 'IDLE 33 22', 'SETTINGS'])
        self.assertEqual(report['steps'][-1]['error'], '')  # KeyboardInterrupt has no message.

    def test_failed_close_does_not_prevent_independent_preference_inspection(self):
        for elapsed in (0, 31):
            with self.subTest(close_timeout_seconds=elapsed):
                fake = CleanupTransport(close_timeout=elapsed)
                code, report = self.run_cleanup_case(fake)
                self.assertEqual(code, 2)
                self.assertEqual(report['status'], 'inconclusive')
                self.assertIn('uncertain', report['cleanup']['input_capture'])
                self.assertEqual(report['cleanup']['preferences'], 'verified')
                self.assertEqual(fake.settings, fake.original)
                close = fake.commands.index('HARNESS 7 CLOSE')
                self.assertEqual(fake.commands[close + 1], 'SETTINGS')
                self.assertEqual(fake.commands.count('HARNESS 7 CLOSE'), 1)

    def test_uncertain_preference_restore_is_never_replayed(self):
        restores = ['BRIGHTNESS 61', 'TIMEZONE -180', 'IDLE 33 22']
        for failed in restores:
            for applied in (False, True):
                with self.subTest(failed=failed, mutation_applied=applied):
                    fake = CleanupTransport(restore_timeout=failed, apply_before_timeout=applied)
                    code, report = self.run_cleanup_case(fake)
                    self.assertEqual(code, 2)
                    self.assertEqual(report['status'], 'inconclusive')
                    self.assertIn('uncertain', report['cleanup']['preferences'])
                    self.assertEqual(fake.commands.count(failed), 1)
                    self.assertTrue(all(fake.commands.count(command) <= 1 for command in restores))
                    self.assertEqual(report['cleanup']['input_capture'], 'closed')

    def test_partial_av_startup_releases_started_and_partially_started_capture_and_usb(self):
        events = []
        class Capture:
            def __init__(self, kind):
                self.kind = kind
            def preflight(self):
                events.append((self.kind, 'preflight'))
            def start(self):
                events.append((self.kind, 'start'))
                if self.kind == 'microphone':
                    raise OSError('fixture playback could not start after capture launch')
            def finish(self, cancel=False):
                events.append((self.kind, 'finish', cancel))
                return {'status': 'skipped'}
        fake = CleanupTransport()
        code, report = self.run_cleanup_case(fake, [Capture('camera'), Capture('microphone')])
        self.assertEqual(code, 1)
        self.assertEqual(events, [('camera', 'preflight'), ('microphone', 'preflight'),
            ('camera', 'start'), ('microphone', 'start'),
            ('camera', 'finish', True), ('microphone', 'finish', True)])
        self.assertEqual(report['cleanup']['input_capture'], 'closed')
        self.assertEqual(report['cleanup']['preferences'], 'verified')
        self.assertEqual(fake.settings, fake.original)
        self.assertTrue(all(step['status'] == 'skipped' for step in report['steps']))
        self.assertEqual(len(report['captures']), 2)

    def test_stale_session_cannot_complete_operation(self):
        class Fake:
            virtual = True
            request_id = 1
            def terminal_command(self, *_args, **_kwargs):
                return {'status': 'OK', 'data': 'boot=1 session=2'}
        with tempfile.TemporaryDirectory() as directory:
            runner = Runner(Fake(), Path(directory))
            runner.boot, runner.session = '1', '3'
            try:
                with self.assertRaisesRegex(RuntimeError, 'stale session'):
                    runner.operation('PING')
            finally:
                runner.trace.close()

    def test_timed_sequences_are_bounded_and_end_released(self):
        events = events_for({'gesture': 'swipe', 'x': 0, 'y': 0, 'to_x': 239, 'to_y': 319})
        self.assertEqual(events[-1]['event'], 'UP')
        self.assertEqual(events[-2]['event'], 'MOVE 239 319')
        self.assertEqual(len(events), 10)

    def test_recovery_does_not_replay_uncertain_restart_or_drop_lock(self):
        connection = Real('/dev/fake', 'unused')
        connection.fd, connection.lock, connection.identity = 5, object(), {'serial': 'test'}
        saved_lock = connection.lock
        replies = iter([TimeoutError('lost mutation reply'), {'status': 'OK', 'data': 'board=c606'}])
        sent = []
        def exchange(command, **_):
            sent.append(command)
            result = next(replies)
            if isinstance(result, Exception):
                raise result
            return result
        connection.terminal_command = exchange
        with patch('harness_transport.os.close'), patch('harness_transport.open_no_reset', return_value=6), \
             patch('harness_transport.rediscover', return_value='/dev/new'), patch.object(connection, 'delay'):
            result = connection.recover('restart')
        self.assertEqual(sent, ['RESTART', 'INFO'])
        self.assertIs(saved_lock, connection.lock)
        self.assertTrue(result['restart_reply_uncertain'])
        self.assertFalse(result['vbus_removed'])

    def test_disabled_recovery_never_opens_harness_session(self):
        class Fake:
            virtual = True
            request_id = 1
            def recover(self, *_):
                return {'mode': 'restart'}
            def terminal_command(self, command, **_):
                self.request_id += 1
                if command != 'HARNESS CAPS':
                    raise AssertionError(command)
                return {'status': 'OK', 'data': 'boot=2 input=unsupported capture=unsupported'}
        with tempfile.TemporaryDirectory() as directory:
            runner = Runner(Fake(), Path(directory))
            runner.boot = '1'
            try:
                self.assertTrue(runner.step({'op': 'recover', 'mode': 'restart'}, 0)['boot_changed'])
            finally:
                runner.trace.close()



    def test_foundation_preflight_bounds_and_owned_stop(self):
        start = dict(op='foundation', action='start', duration_seconds=300)
        stop = dict(op='foundation', action='stop')
        preflight({'steps': [start, stop]})
        for seconds in (True, 299, 601, '300', None):
            with self.assertRaises(ValueError):
                preflight({'steps': [dict(start, duration_seconds=seconds)]})
        for steps in ([stop], [start, start], [start, stop, stop],
                      [dict(start, command='ERASE')],
                      [dict(op='command', command='FOUNDATION LOG START 300')]):
            with self.assertRaises(ValueError):
                preflight({'steps': steps})

    def test_foundation_ownership_and_uncertain_replies(self):
        class Fake:
            virtual = False
            request_id = 1
            state = 'Idle'
            lose_start = lose_stop = apply = False
            def __init__(self): self.commands = []
            def terminal_command(self, command, **_):
                self.commands.append(command)
                self.request_id += 1
                if command == 'INFO':
                    return {'status': 'OK', 'data': 'cycling=true recording=' + str(self.state == 'Recording').lower()}
                if command == 'FOUNDATION LOG STATUS':
                    return {'status': 'OK', 'data': 'Snapshot { status: ' + self.state + ', recording: ' + str(self.state == 'Recording').lower() + ' }'}
                if command.startswith('FOUNDATION LOG START'):
                    if not self.lose_start or self.apply: self.state = 'Recording'
                    if self.lose_start: raise TimeoutError('START reply lost')
                elif command == 'FOUNDATION LOG STOP':
                    if not self.lose_stop or self.apply: self.state = 'Stopped'
                    if self.lose_stop: raise TimeoutError('STOP reply lost')
                else: raise AssertionError(command)
                return {'status': 'ACCEPTED', 'data': ''}
        start = dict(op='foundation', action='start', duration_seconds=300)
        for lost in ('none', 'start', 'stop'):
            for applied in (False, True):
                fake = Fake()
                fake.lose_start, fake.lose_stop, fake.apply = lost == 'start', lost == 'stop', applied
                with tempfile.TemporaryDirectory() as directory:
                    runner = Runner(fake, Path(directory))
                    try:
                        with self.assertRaises(ValueError): runner.foundation(start)
                        runner.foundation_idle(runner.command('INFO'))
                        runner.foundation_eligible = True
                        if lost == 'start':
                            with self.assertRaises(TimeoutError): runner.foundation(start)
                        else:
                            self.assertEqual(runner.foundation(start)['status'], 'Recording')
                            for mode in ('restart', 'usb-reset'):
                                with self.assertRaises(ValueError): runner.step(dict(op='recover', mode=mode), 0)
                        if lost == 'stop':
                            with self.assertRaises(TimeoutError): runner.stop_foundation()
                            if not applied:
                                with self.assertRaises(RuntimeError): runner.stop_foundation()
                                self.assertTrue(runner.foundation_start_may_apply)
                            else: runner.stop_foundation()
                        else: runner.stop_foundation()
                        expected_stops = 0 if lost == 'start' and not applied else 1
                        self.assertEqual(fake.commands.count('FOUNDATION LOG STOP'), expected_stops)
                        self.assertFalse(any('ERASE' in command for command in fake.commands))
                    finally: runner.trace.close()
        with tempfile.TemporaryDirectory() as directory:
            fake = Fake()
            runner = Runner(fake, Path(directory))
            try:
                for state in ('Scanning', 'Ready', 'Recording', 'Stopped', 'Full', 'Error'):
                    fake.state = state
                    with self.assertRaises(ValueError): runner.foundation_idle(runner.command('INFO'))
                with self.assertRaises(ValueError): runner.stop_foundation()
                self.assertFalse(any(command.startswith('FOUNDATION LOG START') or command == 'FOUNDATION LOG STOP' for command in fake.commands))
            finally: runner.trace.close()

    def test_ant_allowlist_requires_bounded_scan_and_owned_stop(self):
        scan = dict(op='ant', action='scan', seconds=10)
        preflight({'steps': [scan, dict(op='command', command='ANT DEVICES'), dict(op='ant', action='stop')]})
        for step in [dict(scan, seconds=True), dict(scan, seconds=0), dict(scan, seconds=11),
                     dict(scan, command='ANT CONNECT 40 1 1'), dict(op='ant', action='connect'),
                     dict(op='ant', action='stop'), dict(op='command', command='ANT SCAN 10'),
                     dict(op='command', command='ANT STOP'), dict(op='command', command='ANT DEVICES extra')]:
            with self.assertRaises(ValueError): preflight({'steps': [step]})

    def test_ant_scan_observes_start_and_cleanup_only_stops_owned_scan(self):
        class Fake:
            virtual = True
            request_id = 1
            def __init__(self):
                self.commands = []
                self.states = ['false']
                self.selected = False
                self.lose_scan = False
            def delay(self, _): pass
            def terminal_command(self, command, **_):
                self.commands.append(command)
                self.request_id += 1
                if command == 'ANT':
                    state = self.states.pop(0) if len(self.states) > 1 else self.states[0]
                    return {'status': 'OK', 'data': 'scanning=' + state + (' type=40 link=connected' if self.selected else '')}
                if command == 'ANT SCAN 1':
                    self.states = ['false', 'true', 'false']
                    if self.lose_scan: raise TimeoutError('scan accepted, reply lost')
                elif command == 'ANT STOP': self.states = ['true', 'false']
                else: raise AssertionError(command)
                return {'status': 'ACCEPTED', 'data': ''}
        with tempfile.TemporaryDirectory() as directory:
            fake = Fake()
            runner = Runner(fake, Path(directory))
            try:
                runner.stop_ant()
                self.assertEqual(fake.commands, [])
                fake.selected = True
                with self.assertRaises(ValueError): runner.ant_idle()
                fake.selected = False
                fake.states = ['true']
                with self.assertRaises(ValueError): runner.ant_idle()
                fake.states = ['false']
                runner.ant_idle()
                runner.ant_eligible = True
                self.assertEqual(runner.ant(dict(op='ant', action='scan', seconds=1))['scanning'], 'false')
                self.assertTrue(runner.ant_scan_observed)
                self.assertFalse(runner.ant_scan_may_apply)
                self.assertNotIn('ANT STOP', fake.commands)
                runner.stop_ant()
                self.assertNotIn('ANT STOP', fake.commands)
                # A lost START reply can leave a queued false before active true.
                runner.ant_eligible = True
                runner.ant_scan_observed = False
                fake.lose_scan = True
                with self.assertRaises(TimeoutError): runner.ant(dict(op='ant', action='scan', seconds=1))
                runner.stop_ant()
                self.assertEqual(fake.commands.count('ANT STOP'), 1)
                self.assertFalse(runner.ant_scan_may_apply)
            finally: runner.trace.close()

if __name__ == '__main__':
    unittest.main()
