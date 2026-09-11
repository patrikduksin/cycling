# /// script
# requires-python = ">=3.12"
# ///
"""Capture the running C606 screen over USB into a private PNG."""
import argparse
import fcntl
import os
from pathlib import Path
import struct
import select
import termios
import tty
import time
import zlib

ROOT = Path(__file__).resolve().parents[1]


def checksum(pixels):
    value = 0x811C9DC5
    for pixel in pixels:
        for byte in struct.pack("<H", pixel):
            value = ((value ^ byte) * 0x01000193) & 0xFFFFFFFF
    return value


class Capture:
    def __init__(self):
        self.frame = None
        self.rows = []

    def feed(self, line):
        fields = line.split()
        if not fields or fields[0] != b"CYCLING_SHOT":
            return None
        if len(fields) == 6 and fields[1] == b"BEGIN":
            if fields[3:5] != [b"80", b"106"]:
                raise ValueError("Unsupported screenshot dimensions")
            self.frame, self.expected = fields[2], int(fields[5], 16)
            self.rows = []
        elif len(fields) == 5 and fields[1] == b"ROW" and fields[2] == self.frame:
            if int(fields[3]) != len(self.rows) or len(fields[4]) != 320:
                raise ValueError("Missing or malformed screenshot row")
            self.rows.append([int(fields[4][i:i + 4], 16) for i in range(0, 320, 4)])
        elif len(fields) == 3 and fields[1] == b"END" and fields[2] == self.frame:
            pixels = [pixel for row in self.rows for pixel in row]
            if len(self.rows) != 106 or checksum(pixels) != self.expected:
                raise ValueError("Incomplete screenshot or checksum mismatch")
            return pixels
        return None


def png(pixels):
    """RGB565 to RGB888 with the LCD's exact 3x mapping and edge rows."""
    raw = bytearray()
    for y in range(320):
        raw.append(0)  # PNG filter: none
        for x in range(240):
            pixel = pixels[min(max(y - 1, 0) // 3, 105) * 80 + x // 3]
            r, g, b = pixel >> 11, (pixel >> 5) & 63, pixel & 31
            raw.extend(((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", 240, 320, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def capture(port, timeout):
    # Avoid serial libraries' DTR/RTS transitions, which restart this C606.
    fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    try:
        tty.setraw(fd)
        attributes = termios.tcgetattr(fd)
        attributes[2] = (attributes[2] | termios.CLOCAL | termios.CREAD) & ~termios.HUPCL
        attributes[4] = attributes[5] = termios.B115200
        termios.tcsetattr(fd, termios.TCSANOW, attributes)
        decoder = Capture()
        pending = bytearray()
        deadline = time.monotonic() + timeout
        request_at = 0
        while time.monotonic() < deadline:
            # Opening USB can coincide with boot. Retry until a capture begins.
            if decoder.frame is None and time.monotonic() >= request_at:
                os.write(fd, b"SCREENSHOT\n")
                request_at = time.monotonic() + 2
            if not select.select([fd], [], [], 0.2)[0]:
                continue
            data = os.read(fd, 4096)
            if not data:
                raise RuntimeError("USB disconnected during screenshot")
            pending.extend(data)
            while b"\n" in pending:
                line, _, pending = pending.partition(b"\n")
                pixels = decoder.feed(line)
                if pixels is not None:
                    return pixels, decoder.frame.decode("ascii")
            if len(pending) > 8192:
                raise ValueError("USB line exceeds screenshot protocol limit")
    finally:
        os.close(fd)
    raise RuntimeError("Screenshot timed out; check firmware and close other USB readers")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", default=os.environ.get("CYCLING_PORT", "/dev/ttyACM0"))
    parser.add_argument("--timeout", type=float, default=20)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    directory = ROOT / ".local/screenshots"
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (ROOT / ".local/usb.lock").open("w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("Another cycling device task is using USB") from None
        pixels, frame = capture(args.port, args.timeout)
    output = args.output or directory / f"{time.time_ns()}.png"
    output.write_bytes(png(pixels))
    print(f"Saved {output} (240x320, frame {frame}, checksum verified)")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError) as error:
        raise SystemExit(str(error)) from None
