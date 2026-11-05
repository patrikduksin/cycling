"""EsptoolFlasher against esptool 5.4.0 output text; never runs esptool or opens a port."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import cycling_devtools.device.flasher as flasher
from cycling_devtools.device import firmware

# A made-up address; recorded transcripts never leave the private logs.
MAC = '02:00:00:00:00:01'

HEADER = f"""esptool v5.4.0
Serial port /dev/ttyACM0:
Connecting...
Connected to ESP32-S3 on /dev/ttyACM0:
Chip type:          ESP32-S3 (QFN56) (revision v0.2)
Features:           Wi-Fi, BT 5 (LE), Dual Core + LP Core, 240MHz, Embedded PSRAM 2MB (AP_3v3)
Crystal frequency:  40MHz
USB mode:           USB-Serial/JTAG
MAC:                {MAC}

"""


FLASH_ID = (
    HEADER
    + """Flash Memory Information:
=========================
Manufacturer: 20
Device: 4018
Detected flash size: 16MB
Flash type set in eFuse: quad (4 data lines)

Staying in bootloader.
"""
)

# A classic ESP32 behind a CP2102N: an EUI-less MAC, a 4 MB GigaDevice flash, no security info.
ESP32_FLASH_ID = f"""esptool v5.4.0
Connected to ESP32 on /dev/ttyUSB0:
Chip type:          ESP32-D0WD-V3 (revision v3.1)
Features:           Wi-Fi, BT, Dual Core + LP Core, 240MHz, Vref calibration in eFuse, Coding Scheme None
Crystal frequency:  40MHz
MAC:                {MAC}

Flash Memory Information:
=========================
Manufacturer: c8
Device: 4016
Detected flash size: 4MB

Staying in bootloader.
"""

C606 = flasher.Chip(
    identity=MAC,
    family='ESP32-S3',
    model='ESP32-S3 (QFN56)',
    revision='v0.2',
    features=('Wi-Fi', 'BT 5 (LE)', 'Dual Core + LP Core', '240MHz', 'Embedded PSRAM 2MB (AP_3v3)'),
    crystal_mhz=40,
    flash=flasher.Flash(manufacturer=0x20, device=0x4018, size=0x1000000),
    usb=flasher.Usb(vendor_id=0x303A, product_id=0x1001),
    security=flasher.Security(secure_boot=False, flash_encryption=False),
)


def security_info(secure_boot='Disabled', flash_encryption='Disabled'):
    return (
        HEADER
        + f"""Security Information:
=====================
Flags: 0x00000000 (0b0)
Key Purposes: (0, 0, 0, 0, 0, 0, 12)
  BLOCK_KEY0 - USER/EMPTY
Chip ID: 9
API Version: 0
Secure Boot: {secure_boot}
Flash Encryption: {flash_encryption}
SPI Boot Crypt Count (SPI_BOOT_CRYPT_CNT): 0x0

Staying in bootloader.
"""
    )


VERIFIED = (
    HEADER
    + """Stub flasher is already running. No upload is necessary.

Configuring flash size...
Verifying 0x1000 (4096) bytes at 0x00020000 in flash against 'stock.bin'...
Verification successful (digest matched).

Staying in bootloader.
"""
)

MISMATCHED = VERIFIED.replace('Verification successful (digest matched).', 'Verification failed (digest mismatch).')


class EsptoolFlasherTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.logs = Path(temporary.name)
        self.commands = []
        self.outputs = []
        self.returncode = 0
        self.resolved = []

        def run(command, **_):
            self.commands.append(command)
            output = self.outputs.pop(0) if self.outputs else ''
            return subprocess.CompletedProcess(command, self.returncode, output, '')

        patcher = patch.object(flasher.subprocess, 'run', run)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.usb = flasher.Usb(vendor_id=0x303A, product_id=0x1001)
        usb = patch.object(flasher, 'usb_of', lambda port: self.usb)
        usb.start()
        self.addCleanup(usb.stop)

    def flasher(self, chip='esp32s3'):
        def resolve():
            self.resolved.append('/dev/ttyACM1')
            return '/dev/ttyACM1'

        return flasher.EsptoolFlasher('/dev/ttyACM0', resolve, self.logs, chip)

    def arguments(self, index=-1):
        """The esptool arguments after `-m esptool`."""
        return self.commands[index][3:]

    def test_enter_describes_the_chip_from_flash_id_and_get_security_info(self):
        self.outputs = [FLASH_ID, security_info()]
        chip = self.flasher().enter()
        self.assertEqual(chip, C606)
        self.assertIs(chip.accepts_unsigned, True)
        self.assertEqual(
            [self.arguments(0), self.arguments(1)],
            [
                [
                    '--chip',
                    'esp32s3',
                    '--port',
                    '/dev/ttyACM0',
                    '--before',
                    'usb-reset',
                    '--after',
                    'no-reset',
                    '--no-stub',
                    'flash-id',
                ],
                [
                    '--chip',
                    'esp32s3',
                    '--port',
                    '/dev/ttyACM0',
                    '--before',
                    'no-reset',
                    '--after',
                    'no-reset',
                    '--no-stub',
                    'get-security-info',
                ],
            ],
        )
        self.assertEqual(self.commands[0][:3], [sys.executable, '-m', 'esptool'])

    def test_auto_detects_the_family_and_leaves_security_unknown_where_the_rom_has_none(self):
        self.outputs = [ESP32_FLASH_ID]
        self.usb = flasher.Usb(vendor_id=0x10C4, product_id=0xEA60)
        chip = self.flasher('auto').enter()
        self.assertEqual(len(self.commands), 1)
        self.assertEqual(self.arguments()[:2] + self.arguments()[4:6], ['--chip', 'auto', '--before', 'default-reset'])
        self.assertEqual(
            (chip.family, chip.model, chip.revision, chip.flash, chip.usb, chip.security, chip.accepts_unsigned),
            ('ESP32', 'ESP32-D0WD-V3', 'v3.1', flasher.Flash(0xC8, 0x4016, 0x400000), self.usb, None, None),
        )

    def test_a_chip_with_an_eui64_is_identified_by_its_base_mac(self):
        eui64 = FLASH_ID.replace(
            f'MAC:                {MAC}\n', f'MAC:                {MAC}:ff:fe\nBASE MAC:           {MAC}\n'
        )
        self.outputs = [eui64.replace(MAC, '02:00:00:00:00:02', 1), security_info()]
        self.assertEqual(self.flasher('auto').enter().identity, MAC)

    def test_an_undescribed_chip_refuses(self):
        self.outputs = [FLASH_ID.replace('Detected flash size: 16MB', 'Detected flash size: Unknown')]
        with self.assertRaisesRegex(flasher.FlasherError, '^esptool did not describe the chip$'):
            self.flasher().enter()

    def test_enabled_security_is_reported_per_feature(self):
        for secure_boot, flash_encryption in (('Enabled', 'Disabled'), ('Disabled', 'Enabled')):
            with self.subTest(secure_boot=secure_boot, flash_encryption=flash_encryption):
                self.outputs = [FLASH_ID, security_info(secure_boot, flash_encryption)]
                chip = self.flasher().enter()
                self.assertEqual(
                    chip.security, flasher.Security(secure_boot == 'Enabled', flash_encryption == 'Enabled')
                )
                self.assertIs(chip.accepts_unsigned, False)

    def test_unreadable_security_lines_count_as_enabled(self):
        self.outputs = [FLASH_ID, HEADER + 'Security Information:\n']
        chip = self.flasher().enter()
        self.assertEqual(chip.security, flasher.Security(secure_boot=True, flash_encryption=True))

    def test_uppercase_identity_is_normalised(self):
        self.outputs = [FLASH_ID.replace(MAC, MAC.upper().replace('02', 'AB', 1)), security_info()]
        self.assertEqual(self.flasher().enter().identity, 'ab:00:00:00:00:01')

    def test_missing_identity_refuses_without_naming_anything(self):
        self.outputs = [FLASH_ID.replace(f'MAC:                {MAC}\n', ''), security_info()]
        with self.assertRaisesRegex(flasher.FlasherError, '^Could not establish device identity$'):
            self.flasher().enter()

    def test_failed_run_keeps_output_in_a_private_log_and_out_of_the_error(self):
        self.outputs = [FLASH_ID]
        self.returncode = 2
        with self.assertRaises(flasher.FlasherError) as raised:
            self.flasher().enter()
        self.assertEqual(str(raised.exception), f'esptool failed; inspect private logs in {self.logs}')
        (log,) = self.logs.glob('*-1.log')
        self.assertIn(MAC, log.read_text())

    def test_each_run_counts_into_its_own_log(self):
        target = self.flasher()
        target.read(0x8000, 0x1000, self.logs / 'out.bin')
        target.write(0x8000, self.logs / 'out.bin')
        self.assertEqual(sorted(path.name.split('-')[1] for path in self.logs.glob('*.log')), ['1.log', '2.log'])

    def test_read_asks_esptool_for_the_region(self):
        self.flasher().read(0x8000, 0x8000, Path('/private/before.bin'))
        self.assertEqual(self.arguments()[-4:], ['read-flash', '0x8000', '0x8000', '/private/before.bin'])
        self.assertEqual(self.arguments()[4:8], ['--before', 'no-reset', '--after', 'no-reset'])

    def test_write_keeps_the_images_own_flash_parameters(self):
        self.flasher().write(0x760000, Path('/private/cycling.bin'))
        self.assertEqual(
            self.arguments()[8:],
            [
                'write-flash',
                '--flash-mode',
                'keep',
                '--flash-freq',
                'keep',
                '--flash-size',
                'keep',
                '0x760000',
                '/private/cycling.bin',
            ],
        )

    def test_verify_reports_the_digest_line(self):
        self.outputs = [VERIFIED, MISMATCHED]
        target = self.flasher()
        self.assertTrue(target.verify(0x20000, Path('/private/stock.bin')))
        self.assertEqual(self.arguments()[-3:], ['verify-flash', '0x20000', '/private/stock.bin'])
        self.assertFalse(target.verify(0x20000, Path('/private/stock.bin')))

    def test_reboot_resets_by_watchdog_then_returns_the_boot_output(self):
        def capture(port, seconds, output):
            self.assertEqual((port, seconds), ('/dev/ttyACM0', 3))
            output.write(b'boot: ota_1\r\n')

        with patch.object(flasher, 'monitor', capture):
            self.assertEqual(self.flasher().reboot(3), b'boot: ota_1\r\n')
        self.assertEqual(self.arguments()[4:], ['--before', 'no-reset', '--after', 'watchdog-reset', 'read-mac'])

    def test_reboot_without_usb_returns_none(self):
        with patch.object(flasher, 'monitor', side_effect=FileNotFoundError):
            self.assertIsNone(self.flasher().reboot(3))

    def test_reboot_keeps_what_arrived_before_usb_went(self):
        def capture(port, seconds, output):
            output.write(b'waiting for download\r\n')
            raise OSError

        with patch.object(flasher, 'monitor', capture):
            self.assertEqual(self.flasher().reboot(3), b'waiting for download\r\n')

    def test_a_later_enter_asks_for_the_port_again(self):
        self.outputs = [FLASH_ID, security_info(), FLASH_ID, security_info()]
        target = self.flasher()
        target.enter()
        self.assertEqual((self.resolved, self.arguments()[3]), ([], '/dev/ttyACM0'))
        target.enter()
        self.assertEqual((self.resolved, self.arguments()[3]), (['/dev/ttyACM1'], '/dev/ttyACM1'))

    def test_flasher_for_builds_the_esptool_backend(self):
        chosen = firmware.flasher_for('/dev/ttyACM0', lambda: '/dev/ttyACM0', self.logs, {})
        self.assertIsInstance(chosen, flasher.EsptoolFlasher)
