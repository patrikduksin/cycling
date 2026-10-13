"""Host-only checks for image validation and OTA selection; never opens USB."""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import Mock, patch
import zlib
import cycling_devtools.device.firmware as device


def image():
    header = bytearray(24)
    header[0], header[1], header[12], header[23] = 0xE9, 1, 9, 1
    data = header + struct.pack('<II', 0x40374000, 16) + bytes(range(16))
    checksum = 0xEF
    for value in range(16):
        checksum ^= value
    data += bytes(15) + bytes([checksum])
    return bytes(data) + hashlib.sha256(data).digest()


def metadata():
    data = bytearray(b'\xff' * 0x8000)
    layout = [
        (1, 2, 0x9000, 0x4000),
        (1, 0, 0xD000, 0x2000),
        (1, 1, 0xF000, 0x1000),
        (1, 3, 0x10000, 0x10000),
        (0, 0x10, 0x20000, 0x73A000),
        (0, 0x11, 0x760000, 0x73A000),
    ]
    for i, (kind, subtype, offset, size) in enumerate(layout):
        data[i * 32 : (i + 1) * 32] = struct.pack('<HBBII16sI', 0x50AA, kind, subtype, offset, size, b'test', 0)
    data[192:224] = b'\xeb\xeb' + b'\xff' * 14 + hashlib.md5(data[:192]).digest()
    for offset, seq in [(0x5000, 3), (0x6000, 2)]:
        struct.pack_into('<I', data, offset, seq)
        struct.pack_into('<I', data, offset + 28, zlib.crc32(data[offset : offset + 4], 0xFFFFFFFF))
    return bytes(data)


class OwnedSnapshotTests(unittest.TestCase):
    def test_flash_snapshot_reads_current_journals_in_a_new_file(self):
        target = object.__new__(device.Device)
        target.read = Mock(return_value=(b'', ''))
        with patch('cycling_devtools.device.firmware.time.time_ns', return_value=123):
            address, path = target.snapshot_owned()
        self.assertEqual(address, device.SLOTS[1] + device.APP_SIZE)
        self.assertEqual(path.name, 'owned-before-123.bin')
        target.read.assert_called_once_with(address, device.SETTINGS_SIZE + device.RIDE_SIZE, path.name)


class Formats(unittest.TestCase):
    def test_reboot_uses_watchdog_to_leave_usb_download_mode(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(device, 'LOCAL', Path(temp)):
            target = device.Device('unused')
            target.run = Mock()
            with patch.object(device, 'monitor'):
                target.reboot()
            target.run.assert_called_once_with('get-security-info', after='watchdog-reset')

    def test_reboot_rejects_rom_download_capture(self):
        def capture(port, seconds, output):
            output.write(b'rst:0x15 (USB_UART_CHIP_RESET),boot:0x0 (DOWNLOAD(USB/UART0))\r\nwaiting for download\r\n')

        with tempfile.TemporaryDirectory() as temp, patch.object(device, 'LOCAL', Path(temp)):
            target = device.Device('unused')
            target.run = Mock()
            with patch.object(device, 'monitor', side_effect=capture):
                with self.assertRaisesRegex(RuntimeError, 'download mode'):
                    target.reboot()

    def test_reboot_reports_usb_disappearance_without_claiming_capture(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(device, 'LOCAL', Path(temp)):
            target = device.Device('unused')
            target.run = Mock()
            with patch.object(device, 'monitor', side_effect=FileNotFoundError), patch('builtins.print') as output:
                target.reboot()
            self.assertIn('USB boot capture unavailable', output.call_args.args[0])

    def test_valid_image_and_extent(self):
        good = image()
        self.assertEqual(device.image_length(good + b'\xff' * 32), len(good))

    def test_corrupt_image_is_rejected(self):
        for offset in [0, 12, 35, -1]:
            bad = bytearray(image())
            bad[offset] ^= 1
            with self.assertRaises(RuntimeError):
                device.image_length(bad)

    def test_truncated_image_is_rejected(self):
        for length in [0, 23, 31, 40, len(image()) - 1]:
            with self.assertRaises(RuntimeError):
                device.image_length(image()[:length])

    def test_application_limit_reserves_ride_region_and_settings_tail(self):
        self.assertEqual(device.APP_SIZE, 0x638000)
        self.assertEqual(device.SLOTS[1] + device.APP_SIZE, 0xD98000)
        self.assertEqual(device.SLOTS[1] + device.APP_SIZE + device.RIDE_SIZE, 0xE98000)
        self.assertEqual(device.SLOTS[1] + device.SLOT_SIZE, 0xE9A000)
        with patch.object(device, 'image_length', return_value=device.APP_SIZE):
            device.validate_candidate(bytes(device.APP_SIZE))
        with patch.object(device, 'image_length', return_value=device.APP_SIZE + 1):
            with self.assertRaises(RuntimeError):
                device.validate_candidate(bytes(device.APP_SIZE + 1))

    def test_partition_table_digest_is_required(self):
        good = metadata()
        device.partitions(good)
        bad = bytearray(good)
        bad[20] ^= 1
        with self.assertRaises(RuntimeError):
            device.partitions(bad)

    def test_different_partition_layout_is_rejected(self):
        bad = bytearray(metadata())
        struct.pack_into('<I', bad, 4 * 32 + 4, 0x30000)
        bad[208:224] = hashlib.md5(bad[:192]).digest()
        with self.assertRaises(RuntimeError):
            device.partitions(bad)

    def test_invalid_record_is_excluded(self):
        bad = bytearray(metadata())
        struct.pack_into('<I', bad, 0x5000 + 24, 3)
        self.assertEqual(device.records(bad), [(2, 0x6000)])

    def test_selection_changes_only_one_sector_and_can_return_to_stock(self):
        class Fake(device.Device):
            def __init__(self):
                self.memory = metadata()
                self.writes = []
                self.reboots = 0

            def write(self, address, path):
                self.writes.append(address)
                block = path.read_bytes()
                self.memory = self.memory[: address - 0x8000] + block + self.memory[address - 0x8000 + len(block) :]

            def read(self, address, size, name):
                return self.memory, ''

            def reboot(self):
                self.reboots += 1

        fake = Fake()
        original = fake.memory
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.object(device, 'LOCAL', Path(temp)),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            for target in [1, 0, 1, 1, 0]:
                fake.select(fake.memory, target)
                self.assertEqual((max(device.records(fake.memory))[0] - 1) % 2, target)
                self.assertEqual(fake.memory[:0x5000], original[:0x5000])
                self.assertEqual(fake.memory[0x7000:], original[0x7000:])
        self.assertEqual(fake.reboots, 5)
        self.assertTrue(all(address in (0xD000, 0xE000) for address in fake.writes))


class BaselineTests(unittest.TestCase):
    """Device.baseline must prove device, backup and protected regions before any write."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.local = Path(temporary.name)
        local_patch = patch.object(device, 'LOCAL', self.local)
        local_patch.start()
        self.addCleanup(local_patch.stop)
        self.backup = bytes(range(256)) * 0x90  # Bootloader region plus partition table.
        self.stock = image()
        self.boot = self.backup[:0x8000]
        (self.local / 'flash.bin').write_bytes(self.backup)
        (self.local / 'stock.bin').write_bytes(self.stock)
        self.manifest = {
            'identity': 'aa:bb:cc:dd:ee:ff',
            'flash_sha256': device.sha(self.backup),
            'stock_sha256': device.sha(self.stock),
            'stock_length': len(self.stock),
            'bootloader_region_sha256': device.sha(self.boot),
        }
        self.write_manifest()
        self.target = device.Device('unused')
        self.target.read = Mock(side_effect=lambda address, size, name: (self.boot, ''))
        self.target.verify = Mock()

    def write_manifest(self):
        (self.local / 'manifest.json').write_text(json.dumps(self.manifest))

    def baseline(self, identity='aa:bb:cc:dd:ee:ff', metadata=None):
        metadata = self.backup[0x8000:0x9000] if metadata is None else metadata
        self.target.baseline(identity, metadata)

    def test_matching_device_reads_bootloader_and_verifies_stock_slot(self):
        self.baseline()
        self.target.read.assert_called_once_with(0, 0x8000, 'bootloader-check.bin')
        self.target.verify.assert_called_once_with(device.SLOTS[0], self.local / 'stock.bin')

    def test_missing_backup_manifest_refuses_before_device_access(self):
        (self.local / 'manifest.json').unlink()
        with self.assertRaisesRegex(RuntimeError, 'Run mise run backup'):
            self.baseline()
        self.target.read.assert_not_called()

    def test_each_mismatch_refuses_before_stock_verification(self):
        cases = {
            'does not match the backup': lambda: self.baseline(identity='11:22:33:44:55:66'),
            'Backup integrity': lambda: (
                (self.local / 'flash.bin').write_bytes(b'x' + self.backup[1:]),
                self.baseline(),
            ),
            'Partition table changed': lambda: self.baseline(metadata=b'\0' * 0x1000),
            'Bootloader changed': lambda: (setattr(self, 'boot', b'\0' * 0x8000), self.baseline()),
        }
        for message, run in cases.items():
            with self.subTest(message=message):
                (self.local / 'flash.bin').write_bytes(self.backup)
                self.boot = self.backup[:0x8000]
                self.target.verify.reset_mock()
                with self.assertRaisesRegex(RuntimeError, message):
                    run()
                self.target.verify.assert_not_called()

    def test_tampered_stock_copy_is_rejected(self):
        (self.local / 'stock.bin').write_bytes(self.stock[:-1] + b'\0')
        with self.assertRaisesRegex(RuntimeError, 'Backup integrity'):
            self.baseline()
        self.target.verify.assert_not_called()


class MainOrderingTests(unittest.TestCase):
    """main() must validate before connecting and verify every protected region after writing."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / '.local/device').mkdir(parents=True)
        self.candidate = self.root / '.local/cycling.bin'
        self.candidate.write_bytes(image())
        self.calls = []
        calls = self.calls
        self.owned = self.root / '.local/device/owned-before.bin'
        owned = self.owned

        class Recorder:
            def __init__(self, port):
                calls.append(('open', port))

            def connect(self):
                calls.append(('connect',))
                return b'metadata', 'identity'

            def backup(self, metadata, identity):
                calls.append(('backup', metadata, identity))

            def baseline(self, identity, metadata):
                calls.append(('baseline', identity, metadata))

            def snapshot_owned(self):
                calls.append(('snapshot',))
                return 0xD98000, owned

            def write(self, address, path):
                calls.append(('write', address, path))

            def verify(self, address, path):
                calls.append(('verify', address, path))

            def select(self, metadata, slot):
                calls.append(('select', slot))

        for target, value in (('ROOT', self.root), ('LOCAL', self.root / '.local/device'), ('Device', Recorder)):
            patcher = patch.object(device, target, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def main(self, action):
        with patch('sys.argv', ['firmware', action, '--port', '/dev/fake']), contextlib.redirect_stdout(io.StringIO()):
            device.main()

    def test_flash_writes_only_after_baseline_and_snapshot_then_verifies_all_regions(self):
        self.main('flash')
        local = self.root / '.local/device'
        self.assertEqual(
            self.calls,
            [
                ('open', '/dev/fake'),
                ('connect',),
                ('baseline', 'identity', b'metadata'),
                ('snapshot',),
                ('write', device.SLOTS[1], self.candidate),
                ('verify', device.SLOTS[1], self.candidate),
                ('verify', device.SLOTS[0], local / 'stock.bin'),
                ('verify', 0xD98000, self.owned),
                ('select', 1),
            ],
        )

    def test_invalid_or_missing_image_never_connects(self):
        bad = bytearray(image())
        bad[-1] ^= 1
        self.candidate.write_bytes(bad)
        with self.assertRaises(RuntimeError):
            self.main('flash')
        self.candidate.unlink()
        with self.assertRaises(FileNotFoundError):
            self.main('flash')
        self.assertNotIn(('connect',), self.calls)

    def test_stock_selects_slot_a_after_baseline_without_writing(self):
        self.main('stock')
        self.assertEqual(
            self.calls, [('open', '/dev/fake'), ('connect',), ('baseline', 'identity', b'metadata'), ('select', 0)]
        )

    def test_backup_never_checks_baseline_or_writes(self):
        self.main('backup')
        self.assertEqual(self.calls, [('open', '/dev/fake'), ('connect',), ('backup', b'metadata', 'identity')])


class SharedUsbLockTests(unittest.TestCase):
    def test_firmware_cannot_start_while_terminal_holds_existing_lock(self):
        from cycling_devtools.terminal import usb

        with tempfile.TemporaryDirectory() as directory, patch.object(usb, 'ROOT', Path(directory)):
            with usb.acquire_lock(), patch.object(device, 'main') as operation:
                with self.assertRaisesRegex(RuntimeError, 'Another cycling device task'):
                    device.run_locked()
                operation.assert_not_called()
            self.assertTrue((Path(directory) / '.local/usb.lock').is_file())

    def test_firmware_excludes_terminal_mmc_logs_and_releases_after_failure(self):
        from cycling_devtools.terminal import usb, logs
        from cycling_devtools.device import mmc_maintenance

        with tempfile.TemporaryDirectory() as directory, patch.object(usb, 'ROOT', Path(directory)):

            def operation():
                for acquire in (usb.acquire_lock, logs.acquire_lock, mmc_maintenance.acquire_lock):
                    with self.assertRaises(BlockingIOError):
                        acquire()
                raise ValueError('operation failed')

            with patch.object(device, 'main', side_effect=operation):
                with self.assertRaisesRegex(ValueError, 'operation failed'):
                    device.run_locked()
            with usb.acquire_lock():
                pass


if __name__ == '__main__':
    unittest.main()
