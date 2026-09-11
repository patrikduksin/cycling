import tempfile
import unittest
from pathlib import Path

from debug import Device, Recording
from screenshot import checksum


class RecordingTests(unittest.TestCase):
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
