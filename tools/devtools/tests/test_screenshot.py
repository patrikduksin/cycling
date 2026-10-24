import argparse
import struct
import tempfile
import unittest
import zlib
from pathlib import Path

from cycling_devtools.harness.runner import Runner, png_rgb565
from cycling_devtools.harness.screenshot import button, screenshot

WIDTH, HEIGHT = 4, 3
# Two flat bands with one odd pixel, as screens are mostly flat colours.
PIXELS = [0xF800] * 5 + [0x07E0] + [0x001F] * 6
FRAME = struct.pack(f'<{len(PIXELS)}H', *PIXELS)


def png_rows(path):
    encoded = path.read_bytes()
    pos, payload, header = 8, b'', None
    while pos < len(encoded):
        size = struct.unpack_from('>I', encoded, pos)[0]
        kind, data = encoded[pos + 4 : pos + 8], encoded[pos + 8 : pos + 8 + size]
        if zlib.crc32(kind + data) != struct.unpack_from('>I', encoded, pos + 8 + size)[0]:
            raise AssertionError('PNG chunk CRC mismatch')
        if kind == b'IHDR':
            header = struct.unpack('>II', data[:8])
        if kind == b'IDAT':
            payload += data
        pos += size + 12
    width, height = header
    raw = zlib.decompress(payload)
    stride = 1 + width * 3
    rows = [raw[y * stride : (y + 1) * stride] for y in range(height)]
    if any(row[0] != 0 for row in rows):
        raise AssertionError('unexpected PNG filter')
    return width, height, [row[1:] for row in rows]


class FakeHarness:
    """Speaks the device's HARNESS capture protocol over one in-memory frame."""

    virtual = True

    def __init__(self, capture='supported', chunk_pixels=4):
        self.capture, self.chunk_pixels = capture, chunk_pixels
        self.commands, self.request_id, self.status_polls = [], 1, 0
        self.input_state = 'idle'

    def delay(self, _seconds):
        pass

    def terminal_command(self, command, **_):
        self.commands.append(command)
        self.request_id += 1
        words = command.split()
        common = 'boot=9 session=5'
        if command in ('ACTIVITY', 'SETTINGS'):
            data = ''
        elif command == 'HARNESS CAPS':
            data = f'capture={self.capture} input=supported chunk_bytes={self.chunk_pixels * 2} buttons=0,1,2,3, boot=9'
        elif words[:2] == ['HARNESS', 'OPEN']:
            data = f'nonce={words[2]} boot=9 session=5'
        elif words[2:] == ['CLOSE']:
            data = 'boot=9 session=0'
        elif words[2] == 'INPUT':
            if words[3] == 'RUN':
                self.input_state = 'completed'
                return {'status': 'ACCEPTED', 'data': common}
            if words[3] == 'ADD':
                return {'status': 'ACCEPTED', 'data': common}
            data = f'{common} state={self.input_state} accepted=1 delivered=1 losses=0 held=false'
        elif words[2:4] == ['CAPTURE', 'START']:
            return {'status': 'ACCEPTED', 'data': common}
        elif words[2:4] == ['CAPTURE', 'STATUS']:
            self.status_polls += 1
            # The first poll arrives before the owner has drawn the next frame.
            data = f'{common} ' + ('state=running frames=' if self.status_polls == 1 else 'state=completed frames=3,')
        elif words[2:4] == ['CAPTURE', 'META']:
            data = (
                f'{common} frame=3 width={WIDTH} height={HEIGHT} format=RGB565LE bytes={len(FRAME)} '
                f'crc32={zlib.crc32(FRAME):08x} started_ms=100 ended_ms=142 complete=true submitted=true'
            )
        elif words[2:4] == ['CAPTURE', 'READ']:
            offset, length = int(words[5]), int(words[6])
            payload = FRAME[offset : offset + length]
            # Emit RLE runs, truncated like the device to a bounded payload of two runs.
            runs, index = [], 0
            while index < len(payload) and len(runs) < 2:
                pixel, count = payload[index : index + 2], 0
                while index < len(payload) and payload[index : index + 2] == pixel:
                    index, count = index + 2, count + 1
                runs.append(struct.pack('<H', count) + pixel)
            decoded = payload[:index]
            data = (
                f'{common} frame=3 offset={offset} length={len(decoded)} encoding=rle565 '
                f'crc32={zlib.crc32(decoded):08x} hex={b"".join(runs).hex()}'
            )
        else:
            data = common
        return {'status': 'OK', 'data': data}


class ScreenshotTests(unittest.TestCase):
    def run_tool(self, fake, **options):
        with tempfile.TemporaryDirectory() as directory:
            runner = Runner(fake, Path(directory))
            path = Path(directory) / 'shot.png'
            try:
                result = screenshot(runner, path, **options)
                return result, png_rows(path)
            finally:
                runner.trace.close()

    def test_reassembles_bounded_chunks_into_scaled_png(self):
        fake = FakeHarness()
        result, (width, height, rows) = self.run_tool(fake, scale=2)
        self.assertEqual((width, height), (8, 6))
        self.assertEqual(result['render_ms'], 42)
        red, green, blue = b'\xff\0\0', b'\0\xff\0', b'\0\0\xff'
        self.assertEqual(rows[0], red * 8)
        self.assertEqual(rows[1], rows[0])
        self.assertEqual(rows[2], red * 2 + green * 2 + blue * 4)
        self.assertEqual(rows[5], blue * 8)
        reads = [c for c in fake.commands if ' CAPTURE READ ' in c]
        self.assertEqual(result['reads'], len(reads))
        # The second read is truncated after two runs; the next one resumes at its decoded end.
        self.assertEqual([int(c.split()[5]) for c in reads], [0, 8, 12, 20])
        self.assertEqual(fake.commands[-1], 'HARNESS 5 CLOSE')

    def test_presses_buttons_before_capture(self):
        fake = FakeHarness()
        self.run_tool(fake, presses=[1, 2])
        self.assertLess(fake.commands.index('ACTIVITY'), next(i for i, c in enumerate(fake.commands) if ' INPUT ' in c))
        adds = [c for c in fake.commands if ' INPUT ADD ' in c]
        self.assertEqual([c.split()[-1] for c in adds], ['1', '2'])
        first_capture = next(i for i, c in enumerate(fake.commands) if 'CAPTURE START' in c)
        last_input = max(i for i, c in enumerate(fake.commands) if ' INPUT ' in c)
        self.assertLess(last_input, first_capture)

    def test_refuses_builds_without_capture(self):
        fake = FakeHarness(capture='unsupported')
        with self.assertRaisesRegex(RuntimeError, 'CYCLING_HARNESS=1'):
            self.run_tool(fake)
        self.assertNotIn('OPEN', ' '.join(fake.commands))

    def test_frame_crc_mismatch_is_rejected_and_session_closed(self):
        fake = FakeHarness()
        serve = fake.terminal_command

        def tampered(command, **kwargs):
            # META advertises a different whole-frame CRC than the chunks reassemble to.
            reply = serve(command, **kwargs)
            if ' CAPTURE META ' in command:
                reply['data'] = reply['data'].replace(f'crc32={zlib.crc32(FRAME):08x}', 'crc32=00000000')
            return reply

        fake.terminal_command = tampered
        with self.assertRaisesRegex(ValueError, 'integrity'):
            self.run_tool(fake)
        self.assertEqual(fake.commands[-1], 'HARNESS 5 CLOSE')

    def test_png_scale_bounds(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                png_rgb565(Path(directory) / 'x.png', b'\0\0', 1, 1, scale=0)

    def test_button_names(self):
        self.assertEqual(button('bottom-left'), 1)
        self.assertEqual(button('3'), 3)
        with self.assertRaises(argparse.ArgumentTypeError):
            button('4')


if __name__ == '__main__':
    unittest.main()
