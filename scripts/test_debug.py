import tempfile
import unittest
from pathlib import Path

from debug import (Device, Recording, finish_created_ride, restore_ride_view,
                   select_demo_ride)
from screenshot import checksum


class RecordingTests(unittest.TestCase):
    def test_demo_view_is_restored_before_finish_target_is_used(self):
        class Fake:
            def __init__(self):
                self.actions = []

            def command(self, command):
                self.actions.append(command)

            def tap(self, x, y):
                self.actions.append(f'TAP {x} {y}')

            def expect(self, wanted):
                self.actions.append(('EXPECT', wanted))
                return wanted

        device = Fake()
        restore_ride_view(device, {'ride_page': 0, 'ride_layout': 1})
        self.assertEqual(device.actions[:2], ['BUTTON 1 1', 'TAP 80 220'])
        self.assertEqual(device.actions[-1][1]['screen'], 'ride')

    def test_demo_scenario_temporarily_changes_live_selection(self):
        class Fake:
            def __init__(self):
                self.taps = []

            def tap(self, x, y):
                self.taps.append((x, y))

            def expect(self, wanted):
                return {**wanted, 'ride_selected_source': 'demo'}

        device = Fake()
        state = select_demo_ride(device, {'ride_selected_source': 'live'})
        self.assertEqual(device.taps, [(80, 220)])
        self.assertEqual(state['ride_selected_source'], 'demo')
        self.assertEqual(select_demo_ride(device, state), state)

    def test_created_ride_cleanup_is_bounded_by_current_phase(self):
        class Fake:
            def __init__(self, phase):
                self.phase = phase
                self.commands = []

            def command(self, command, timeout=0):
                self.commands.append((command, timeout))
                if command == 'RIDE PAUSE':
                    self.phase = 'paused'
                elif command == 'RIDE FINISH':
                    self.phase = 'saved'
                return {'ride_recording': self.phase}

        running = Fake('recording')
        finish_created_ride(running)
        self.assertEqual([command for command, _ in running.commands],
                         ['STATE', 'RIDE PAUSE', 'RIDE FINISH'])
        saved = Fake('saved')
        finish_created_ride(saved)
        self.assertEqual([command for command, _ in saved.commands], ['STATE'])

    def key(self, decoder, color=0):
        pixels = [color] * 8480
        decoder.feed(f'CYCLING_REC BEGIN 10 0 1 100 1 {checksum(pixels):08x}'.encode())
        for row in range(106):
            decoder.feed(f'CYCLING_REC ROW 10 0 {row} 50{color:04x}'.encode())
        return decoder.feed(b'CYCLING_REC END 10 0')

    def test_key_delta_and_unchanged_frame(self):
        decoder = Recording()
        metadata, pixels = self.key(decoder)
        self.assertEqual(metadata['ms'], 100)
        self.assertEqual(pixels, [0] * 8480)
        pixels[:80] = [0xffff] * 80
        decoder.feed(f'CYCLING_REC BEGIN 10 1 6 300 0 {checksum(pixels):08x}'.encode())
        decoder.feed(b'CYCLING_COMPANION valid=10')
        decoder.feed(b'CYCLING_REC ROW 10 1 0 50ffff')
        self.assertEqual(decoder.feed(b'CYCLING_REC END 10 1')[1], pixels)
        decoder.feed(f'CYCLING_REC BEGIN 10 2 11 500 0 {checksum(pixels):08x}'.encode())
        self.assertEqual(decoder.feed(b'CYCLING_REC END 10 2')[1], pixels)
        decoder.feed(b'CYCLING_REC STOP 10 3')
        self.assertEqual(decoder.stopped, {10})

    def test_rejects_lost_frames_rows_and_bad_checksums(self):
        for error in [b'CYCLING_REC BEGIN 10 2 6 300 0 00000000', b'CYCLING_REC STOP 10 2']:
            decoder = Recording()
            self.key(decoder)
            with self.assertRaises(ValueError):
                decoder.feed(error)
        decoder = Recording()
        decoder.feed(b'CYCLING_REC BEGIN 10 0 1 100 1 00000000')
        with self.assertRaises(ValueError):
            decoder.feed(b'CYCLING_REC END 10 0')
        decoder = Recording()
        self.key(decoder)
        decoder.feed(b'CYCLING_REC BEGIN 10 1 6 300 0 00000000')
        with self.assertRaises(ValueError):
            decoder.feed(b'CYCLING_REC END 10 1')

    def test_rejects_invalid_runs_and_duplicate_rows(self):
        for run in [b'00ffff', b'51ffff', b'01ffff', b'zzffff']:
            decoder = Recording()
            decoder.feed(b'CYCLING_REC BEGIN 10 0 1 100 1 00000000')
            with self.assertRaises(ValueError):
                decoder.feed(b'CYCLING_REC ROW 10 0 0 ' + run)
        decoder = Recording()
        decoder.feed(b'CYCLING_REC BEGIN 10 0 1 100 1 00000000')
        decoder.feed(b'CYCLING_REC ROW 10 0 0 50ffff')
        with self.assertRaises(ValueError):
            decoder.feed(b'CYCLING_REC ROW 10 0 0 50ffff')


class StreamTests(unittest.TestCase):
    def device(self, directory):
        device = Device.__new__(Device)
        device.pending = bytearray()
        device.replies = {}
        device.events = [dict(id=7, command='BEGIN')]
        device.recording = Recording()
        device.directory = Path(directory)
        device.frames = []
        return device

    def test_split_reply_resynchronizes_after_torn_frame_prefix(self):
        with tempfile.TemporaryDirectory() as directory:
            for prefix in [
                b'CYCLING_FRA',
                b'CYCLING_FRAME frame=',
                b'CYCLING_FRAME frame=12 render_ms=',
                b'CYCLING_FRAME frame=12 render_ms=1 dra',
                b'CYCLING_FRAME frame=12 render_ms=1 draws=3 skip',
            ]:
                device = self.device(directory)
                reply = prefix + b'CYCLING_DEBUG 7 OK {"screen":"home"}\n'
                device.feed(reply[:17])
                self.assertEqual(device.replies, {})
                device.feed(reply[17:])
                self.assertEqual(device.replies[7], ('OK', {'screen': 'home'}))

    def test_reboot_expiry_and_malformed_replies_still_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            for line, error in [
                (b'CYCLING_BOOT version=x\n', RuntimeError),
                (b'CYCLING_DEBUG expired\n', RuntimeError),
                (b'CYCLING_DEBUG broken\n', ValueError),
            ]:
                device = self.device(directory)
                with self.assertRaises(error):
                    device.feed(line)
