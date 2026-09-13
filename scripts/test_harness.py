import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zlib

from harness import Runner, decode_chunk, events_for, png_rgb565, preflight, execute
from harness_transport import Real


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

    def test_failure_restores_all_unsaved_preferences_without_save(self):
        from types import SimpleNamespace
        class Fake:
            virtual = True
            request_id = 1
            def __init__(self, *_args):
                self.settings = {'brightness': '61', 'timezone': '-180', 'dim_timeout': '33', 'dim_brightness': '22'}
                self.commands = []
                self.closed = False
            def __enter__(self):
                return self
            def __exit__(self, *_):
                self.closed = True
            def terminal_command(self, command, **_):
                self.commands.append(command)
                if command == 'INFO':
                    data = 'cycling=false recording=false'
                elif command == 'HARNESS CAPS':
                    data = 'input=unsupported capture=unsupported boot=1 ordinary=INFO,STATUS,SETTINGS'
                elif command == 'SETTINGS':
                    data = ' '.join(f'{k}={v}' for k,v in self.settings.items())
                elif command.startswith('BRIGHTNESS '):
                    self.settings['brightness'] = command.split()[1]; data = ''
                elif command.startswith('TIMEZONE '):
                    self.settings['timezone'] = command.split()[1]; data = ''
                elif command.startswith('IDLE '):
                    self.settings['dim_timeout'], self.settings['dim_brightness'] = command.split()[1:]; data = ''
                else:
                    data = ''
                return {'status': 'OK', 'data': data}
        fake = Fake()
        original = dict(fake.settings)
        def failing_step(*_):
            fake.settings.update(brightness='50', timezone='0', dim_timeout='30', dim_brightness='10')
            raise TimeoutError('uncertain operation')
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            args = SimpleNamespace(run_id=None, agent_tool_calls=None, backend='virtual', state_dir=None, sdk=False, no_harness=True)
            with patch('harness.Virtual', return_value=fake), patch.object(Runner, 'step', failing_step), patch('sys.stdout', io.StringIO()):
                self.assertEqual(execute(args, {'steps': [{'op': 'command', 'command': 'STATUS'}]}, output), 1)
            self.assertEqual(fake.settings, original)
            self.assertNotIn('SAVE', fake.commands)
            self.assertTrue(fake.closed)
            self.assertEqual(json.loads((output / 'report.json').read_text())['cleanup']['preferences'], 'verified')

    def test_stale_session_cannot_complete_operation(self):
        class Fake:
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


if __name__ == '__main__':
    unittest.main()
