"""cycling flasher against a MemoryFlasher: the verbs, the JSON of info, and where reads may land."""

import contextlib
import hashlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from cycling_devtools.device import flasher_cli
from cycling_devtools.workspace import ROOT
from memory_flasher import MemoryFlasher

IDENTITY = '02:00:00:00:00:07'


class FlasherCliTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.logs = Path(temporary.name)
        self.flasher = MemoryFlasher(bytes(range(256)) * 256, IDENTITY)
        self.flasher.boot_output = b'I (31) boot: Loaded app\r\n'
        self.asked = []

        def flasher_for(port, resolve, log_dir, chip):
            self.asked.append((port, log_dir, chip))
            return self.flasher

        for target, value in (
            ('flasher_for', flasher_for),
            ('selected_port', lambda parser, explicit: '/dev/ttyUSB0'),
            ('LOG_DIR', self.logs),
        ):
            patcher = patch.object(flasher_cli, target, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def run_cli(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            status = flasher_cli.main(list(argv))
        return status, out.getvalue()

    def test_info_prints_the_description_with_an_opaque_device_id(self):
        status, out = self.run_cli('info')
        self.assertEqual(status, 0)
        self.assertNotIn('02:00:00', out)
        described = json.loads(out)
        device_id = described.pop('device_id')
        self.assertNotEqual(device_id, hashlib.sha256(IDENTITY.encode()).hexdigest()[:12])
        self.assertEqual(json.loads(self.run_cli('info')[1])['device_id'], device_id)
        self.assertEqual(
            described,
            {
                'family': 'ESP32-S3',
                'model': 'ESP32-S3 (QFN56)',
                'revision': 'v0.2',
                'features': ['Wi-Fi', 'BT 5 (LE)', 'Dual Core + LP Core', '240MHz'],
                'crystal_mhz': 40,
                'flash': {'manufacturer': '20', 'device': '4018', 'size': 0x1000000},
                'usb': {'vendor_id': '303a', 'product_id': '1001'},
                'security': {'secure_boot': False, 'flash_encryption': False},
                'accepts_unsigned': True,
            },
        )
        self.assertEqual(self.asked[0], ('/dev/ttyUSB0', self.logs, 'auto'))

    def test_a_rom_without_security_info_says_unknown(self):
        self.flasher.security = None
        described = json.loads(self.run_cli('info')[1])
        self.assertEqual((described['security'], described['accepts_unsigned']), (None, None))

    def test_every_verb_ends_with_a_reboot(self):
        self.run_cli('info')
        self.assertEqual((self.flasher.enters, self.flasher.reboots), (1, 1))

    def test_a_failed_verb_still_reboots_the_chip(self):
        image = self.logs / 'image.bin'
        image.write_bytes(b'x')

        def broken(address, path):
            raise RuntimeError('digest timed out')

        self.flasher.verify = broken
        with self.assertRaises(RuntimeError):
            self.run_cli('verify', '0', str(image))
        self.assertEqual((self.flasher.enters, self.flasher.reboots), (1, 1))

    def test_a_failed_verb_through_the_page_leaves_the_reset_to_the_page(self):
        page = flasher_cli.PageFlasher(self.logs / 'door.sock', self.logs)
        page.enter = lambda: (_ for _ in ()).throw(KeyboardInterrupt())
        rebooted = []
        page.reboot = lambda seconds: rebooted.append(seconds)
        self.flasher = page
        with self.assertRaises(KeyboardInterrupt):
            self.run_cli('info')
        self.assertEqual(rebooted, [])

    def test_reboot_seconds_beyond_what_the_page_accepts_are_refused(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            self.run_cli('reboot', '--seconds', '61')
        self.assertEqual(self.flasher.enters, 0)

    def test_read_writes_the_region_under_local(self):
        (ROOT / '.local').mkdir(exist_ok=True)
        scratch = tempfile.TemporaryDirectory(dir=ROOT / '.local')
        self.addCleanup(scratch.cleanup)
        local = Path(scratch.name)
        status, out = self.run_cli('read', '0x100', '0x80', str(local / 'region.bin'))
        self.assertEqual(status, 0)
        self.assertEqual((local / 'region.bin').read_bytes(), bytes(self.flasher.memory[0x100:0x180]))
        self.assertIn('0x80 bytes at 0x100', out)

    def test_read_outside_local_is_refused_before_the_device_is_touched(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            self.run_cli('read', '0', '0x10', str(self.logs / 'out.bin'))
        self.assertEqual(self.flasher.enters, 0)

    def test_verify_exits_1_when_flash_differs(self):
        image = self.logs / 'image.bin'
        image.write_bytes(bytes(self.flasher.memory[0x200:0x240]))
        self.assertEqual(self.run_cli('verify', '0x200', str(image))[0], 0)
        self.assertEqual(self.run_cli('verify', '0x201', str(image))[0], 1)

    def test_reboot_keeps_the_boot_output_in_a_private_log(self):
        status, out = self.run_cli('reboot', '--seconds', '1')
        self.assertEqual(status, 0)
        (log,) = self.logs.glob('*-boot.log')
        self.assertEqual(log.read_bytes(), b'I (31) boot: Loaded app\r\n')
        self.assertIn(str(log), out)

    def test_there_is_no_write_verb(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            self.run_cli('write', '0', 'image.bin')


if __name__ == '__main__':
    unittest.main()
