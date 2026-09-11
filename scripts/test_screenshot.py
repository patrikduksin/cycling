import struct
import unittest
import zlib

from screenshot import Capture, checksum, png


class ScreenshotTests(unittest.TestCase):
    def test_capture_with_interleaved_logs(self):
        pixels = [0xF800, 0x07E0, 0x001F, 0xFFFF] * 2120
        capture = Capture()
        capture.feed(f"CYCLING_SHOT BEGIN 42 80 106 {checksum(pixels):08x}".encode())
        for row in range(106):
            self.assertIsNone(capture.feed(b"CYCLING_WIFI connected"))
            data = ''.join(f'{p:04x}' for p in pixels[row * 80:(row + 1) * 80])
            capture.feed(f"CYCLING_SHOT ROW 42 {row} {data}".encode())
        self.assertEqual(capture.feed(b"CYCLING_SHOT END 42"), pixels)

    def test_dropped_row_and_bad_checksum(self):
        capture = Capture()
        capture.feed(b"CYCLING_SHOT BEGIN 1 80 106 00000000")
        with self.assertRaises(ValueError):
            capture.feed(b"CYCLING_SHOT ROW 1 1 " + b"0000" * 80)
        capture.rows = [[0] * 80] * 106
        with self.assertRaises(ValueError):
            capture.feed(b"CYCLING_SHOT END 1")

    def test_png_colors_and_edge_mapping(self):
        pixels = [0] * (80 * 106)
        pixels[0], pixels[80], pixels[-1] = 0xF800, 0x07E0, 0x001F
        data = png(pixels)
        offset, compressed = 8, bytearray()
        while offset < len(data):
            length = struct.unpack_from('>I', data, offset)[0]
            kind, body = data[offset + 4:offset + 8], data[offset + 8:offset + 8 + length]
            self.assertEqual(zlib.crc32(kind + body), struct.unpack_from('>I', data, offset + 8 + length)[0])
            if kind == b'IDAT':
                compressed.extend(body)
            offset += length + 12
        raw = zlib.decompress(compressed)
        self.assertEqual(len(raw), 721 * 320)
        self.assertEqual(raw[1:10], b'\xff\0\0' * 3)
        self.assertEqual(raw[721 * 3 + 1:721 * 3 + 4], b'\xff\0\0')
        self.assertEqual(raw[721 * 4 + 1:721 * 4 + 4], b'\0\xff\0')
        self.assertEqual(raw[-3:], b'\0\0\xff')
