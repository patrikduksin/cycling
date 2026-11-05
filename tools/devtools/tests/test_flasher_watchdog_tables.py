"""The page flasher's per-family tables equal what the pinned python esptool does.

web/flasher/src/families.ts copies, for every chip family esptool-js knows,
the register and modem-line sequences python esptool runs and esptool-js
lacks: the watchdogs `_post_connect` disables on USB-Serial-JTAG, the
`watchdog_reset` that `--after watchdog-reset` runs on the chip's own USB,
and the `hard_reset` it falls back to on a USB-UART bridge. This runs
python's own methods against a recording chip whose registers all read 0
and a recording port, and compares the traffic with the TypeScript tables,
so the copy is checked, not hand-synced.
"""

import re
import sys
import unittest
from contextlib import ExitStack
from unittest.mock import patch

from esp_pylib.constants import HARDWARE_FLOW_CONTROL_VID_PIDS
from esptool.targets import CHIP_DEFS

from cycling_devtools.workspace import ROOT

TABLES = ROOT / 'web' / 'flasher' / 'src' / 'families.ts'
NUMBER = r'(0x[0-9a-fA-F_]+|\d[\d_]*)'
STEP = re.compile(
    r"\{\s*_tag:\s*'(?P<tag>Write|SetBits|Hold|Rts|Dtr)',\s*"
    rf'(?:address:\s*{NUMBER},\s*(?:value|bits):\s*{NUMBER}(?:,\s*mask:\s*{NUMBER})?'
    rf'|ms:\s*{NUMBER}|asserted:\s*(true|false))\s*,?\s*\}}'
    r'|\.\.\.(?P<spread>[A-Z_]+)'
)
ESPRESSIF = 0x303A
USB_JTAG_SERIAL = (ESPRESSIF, 0x1001)
# A CP2102N, and the CP2102C whose CTS follows the chip's RTS (esp_pylib's flow-control list).
UART_BRIDGE = (0x10C4, 0xEA60)
UART_FLOW_CONTROL = (0x10C4, 0xEA64)
FULL_MASK = 0xFFFFFFFF


def number(text):
    return int(text.replace('_', ''), 0)


def bracketed(source, start):
    """The text between the `[` at start and its matching `]`."""
    depth = 0
    for at in range(start, len(source)):
        depth += {'[': 1, ']': -1}.get(source[at], 0)
        if depth == 0:
            return source[start + 1 : at]
    raise AssertionError(f'unbalanced [ at {start} in {TABLES}')


def steps(body, source):
    found = []
    for match in STEP.finditer(body):
        if match['spread']:
            found += named(match['spread'], source)
            continue
        tag, address, value, mask, ms, asserted = match.group('tag', 2, 3, 4, 5, 6)
        if tag == 'Hold':
            found.append(('Hold', number(ms)))
        elif tag in ('Rts', 'Dtr'):
            found.append((tag, asserted == 'true'))
        elif tag == 'SetBits':
            # A read-modify-write; on a register that reads 0 it writes its bits.
            found += [('Read', number(address)), ('Write', number(address), number(value), FULL_MASK)]
        else:
            found.append(('Write', number(address), number(value), number(mask) if mask else FULL_MASK))
    return found


def named(name, source):
    start = re.search(rf'export const {name}\b[^=]*=\s*\[', source)
    if start is None:
        raise AssertionError(f'{name} not found in {TABLES}')
    return steps(bracketed(source, start.end() - 1), source)


def family_table(family, field):
    source = TABLES.read_text()
    # oxfmt drops the quotes from keys that are identifiers (ESP32, ESP8266).
    block = re.search(rf"^  '?{re.escape(family)}'?: \{{", source, re.M)
    if block is None:
        raise AssertionError(f'{family} not found in {TABLES}')
    start = re.compile(rf'\b{field}:\s*\[').search(source, block.end())
    if start is None:
        raise AssertionError(f'{family}.{field} not found in {TABLES}')
    return steps(bracketed(source, start.end() - 1), source)


class Port:
    """Records modem-line writes; setting RTS re-applies DTR, as esp_pylib's set_rts does."""

    def __init__(self, traffic):
        self.traffic, self.dtr, self.rts, self.port = traffic, False, False, 'recorded'

    def isOpen(self):
        return True

    def setRTS(self, state):
        self.rts = state
        self.traffic.append(('Rts', state))

    def setDTR(self, state):
        self.dtr = state
        self.traffic.append(('Dtr', state))


def recorded(rom, vid_pid, method):
    """What python esptool's `method` sends a `rom` chip seen on `vid_pid`, every register reading 0."""
    traffic = []

    def init(self):
        self._port = Port(traffic)
        self.secure_download_mode = self.sync_stub_detected = False
        self.cache = {'usb_vid': vid_pid[0], 'usb_pid': vid_pid[1], 'security_info': None}

    def read_reg(self, address, timeout=None):
        traffic.append(('Read', address))
        return 0

    def write_reg(self, address, value, mask=FULL_MASK, delay_us=0, delay_after_us=0):
        traffic.append(('Write', address, value, mask))

    chip = type(
        'Recorder',
        (rom,),
        {
            '__init__': init,
            'get_usb_vid_pid': lambda self: vid_pid,
            'read_reg': read_reg,
            'write_reg': write_reg,
            # P4's _post_connect also powers the flash on; that is esptool-js's job too, not a watchdog step.
            'power_on_flash': lambda self: None,
            'get_chip_revision': lambda self: 100,
        },
    )()

    def hold(seconds):
        traffic.append(('Hold', round(seconds * 1000)))

    with ExitStack() as stack:
        stack.enter_context(patch('time.sleep', hold))
        for name, module in list(sys.modules.items()):
            if name.startswith('esptool.targets.') and hasattr(module, 'sleep'):
                stack.enter_context(patch(f'{name}.sleep', hold))
        # The browser cannot clear HUPCL; esp_pylib then deasserts DTR itself, as on Windows.
        stack.enter_context(patch('esp_pylib.serial_reset._set_hupcl', lambda port, enabled: False))
        getattr(chip, method)()
    return traffic


def families():
    return {rom.CHIP_NAME: rom for rom in CHIP_DEFS.values()}


class FamilyTables(unittest.TestCase):
    def test_every_family_python_esptool_knows_has_a_table(self):
        source = TABLES.read_text()
        listed = set(re.findall(r"^  '?([A-Z0-9-]+)'?: \{", source, re.M))
        self.assertEqual(listed, set(families()))

    def test_disable_watchdogs_matches_post_connect_on_usb_serial_jtag(self):
        for family, rom in families().items():
            with self.subTest(family=family):
                self.assertEqual(
                    family_table(family, 'disableWatchdogs'), recorded(rom, USB_JTAG_SERIAL, '_post_connect')
                )

    def test_usb_reboot_matches_watchdog_reset_on_usb_serial_jtag_and_otg(self):
        for family, rom in families().items():
            otg = (ESPRESSIF, rom.IMAGE_CHIP_ID)
            with self.subTest(family=family):
                table = family_table(family, 'usbReboot')
                self.assertEqual(table, recorded(rom, USB_JTAG_SERIAL, 'watchdog_reset'))
                if family == 'ESP32-E22':
                    # python's E22 watchdog_reset and hard_reset call each other on USB-OTG
                    # while GPIO0 reads low; the page uses the USB-Serial-JTAG sequence there.
                    with self.assertRaises(RecursionError):
                        recorded(rom, otg, 'watchdog_reset')
                else:
                    self.assertEqual(table, recorded(rom, otg, 'watchdog_reset'))

    def test_uart_reboot_matches_hard_reset_on_a_usb_uart_bridge(self):
        for family, rom in families().items():
            with self.subTest(family=family):
                before = family_table(family, 'hardReset')
                self.assertEqual(
                    before + named('RTS_PULSE', TABLES.read_text()), recorded(rom, UART_BRIDGE, 'hard_reset')
                )
                self.assertEqual(
                    before + named('RTS_PULSE_FLOW_CONTROL', TABLES.read_text()),
                    recorded(rom, UART_FLOW_CONTROL, 'hard_reset'),
                )

    def test_flow_control_bridges_match_esp_pylib(self):
        source = TABLES.read_text()
        start = re.search(r'export const FLOW_CONTROL_BRIDGES\b[^=]*=\s*\[', source)
        body = bracketed(source, start.end() - 1)
        bridges = re.findall(rf'vendorId:\s*{NUMBER},\s*productId:\s*{NUMBER}', body)
        self.assertEqual({(number(vid), number(pid)) for vid, pid in bridges}, set(HARDWARE_FLOW_CONTROL_VID_PIDS))


if __name__ == '__main__':
    unittest.main()
