"""The page flasher's S3 watchdog tables equal what the pinned python esptool writes.

web/flasher/src/watchdog.ts copies two register sequences esptool-js lacks.
This runs python esptool's own disable_watchdogs() and watchdog_reset()
against a recording chip and compares the writes with the TypeScript tables,
so the copy is checked, not hand-synced.
"""

import re
import unittest
from unittest.mock import patch

from esptool.targets.esp32s3 import ESP32S3ROM

from cycling_devtools.workspace import ROOT

TABLES = ROOT / 'web' / 'flasher' / 'src' / 'watchdog.ts'
NUMBER = r'(0x[0-9a-fA-F_]+|\d[\d_]*)'
STEP = re.compile(
    r"\{\s*_tag:\s*'(Write|SetBits|Hold)',\s*"
    rf'(?:address:\s*{NUMBER},\s*(?:value|bits):\s*{NUMBER}|ms:\s*{NUMBER})\s*,?\s*\}}'
)


def number(text):
    return int(text.replace('_', ''), 0)


def table(name):
    source = TABLES.read_text()
    start = re.search(rf'export const {name}\b[^=]*=\s*\[', source)
    if start is None:
        raise AssertionError(f'{name} not found in {TABLES}')
    body = source[start.end() : source.index('\n]', start.end())]
    steps = []
    for match in STEP.finditer(body):
        tag, address, value, ms = match.groups()
        steps.append((tag, number(ms)) if tag == 'Hold' else (tag, number(address), number(value)))
    return steps


class Recorder(ESP32S3ROM):
    """Records register traffic instead of talking to a chip on 303a:1001; every register reads 0."""

    def __init__(self):
        self.steps = []

    def uses_usb_jtag_serial(self):
        return True

    def read_reg(self, address, timeout=None):
        return 0

    def write_reg(self, address, value, mask=0xFFFFFFFF, delay_us=0, delay_after_us=0):
        self.steps.append(('Write', address, value))


def recorded(run):
    chip = Recorder()
    with patch('esptool.targets.esp32s3.sleep', lambda seconds: chip.steps.append(('Hold', int(seconds * 1000)))):
        run(chip)
    return chip.steps


def as_writes(steps):
    """A SetBits step is a read-modify-write; on a register that reads 0 it writes its bits."""
    return [('Write', step[1], step[2]) if step[0] == 'SetBits' else step for step in steps]


class WatchdogTables(unittest.TestCase):
    def test_disable_watchdogs_matches_python_esptool(self):
        self.assertEqual(as_writes(table('DISABLE_WATCHDOGS')), recorded(ESP32S3ROM.disable_watchdogs))

    def test_watchdog_reset_matches_python_esptool(self):
        self.assertEqual(table('WATCHDOG_RESET'), recorded(ESP32S3ROM.watchdog_reset))


if __name__ == '__main__':
    unittest.main()
