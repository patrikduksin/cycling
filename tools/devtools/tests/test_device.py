"""Host-only checks for images, A/B selection, rollback and stock restore; never opens USB."""

import contextlib
import hashlib
import io
import json
import os
import pty
import struct
import tempfile
import termios
import threading
import unittest
import zlib
from pathlib import Path
from unittest.mock import Mock, patch

import cycling_devtools.device.firmware as device

IDENTITY = 'aa:bb:cc:dd:ee:ff'
# Workspace tests patch console_info; attachment tests call the real one.
REAL_CONSOLE_INFO = device.console_info


def image(project=device.PROJECT, extra=0, marker=0, version=b'0.1.0'):
    """A valid ESP32-S3 image whose application descriptor names project."""
    segment = bytearray(0x60 + extra)
    struct.pack_into('<I', segment, 0, device.APP_DESC_MAGIC)
    segment[0x10 : 0x10 + len(version)] = version
    segment[0x30 : 0x30 + len(project)] = project
    segment[-1] = marker
    header = bytearray(24)
    header[0], header[1], header[12], header[23] = 0xE9, 1, 9, 1
    data = header + struct.pack('<II', 0x3C000020, len(segment)) + segment
    checksum = 0xEF
    for value in segment:
        checksum ^= value
    end = (len(data) + 16) & ~15
    data += bytes(end - len(data) - 1) + bytes([checksum])
    return bytes(data) + hashlib.sha256(data).digest()


def stock_image():
    return image(project=b'Project', extra=0x1800)


def metadata(sequences=((0x5000, 3), (0x6000, 2))):
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
    for offset, seq in sequences:
        struct.pack_into('<I', data, offset, seq)
        struct.pack_into('<I', data, offset + 28, zlib.crc32(data[offset : offset + 4], 0xFFFFFFFF))
    return bytes(data)


def stock_flash():
    """A 16 MiB stock layout: bootloader, partitions, stock in A, stock leftovers in B, selection on A."""
    flash = bytearray(b'\xff' * device.FLASH_SIZE)
    flash[:0x8000] = bytes(range(256)) * 0x80
    flash[0x8000:0x10000] = metadata()
    stock = stock_image()
    flash[device.SLOTS[0] : device.SLOTS[0] + len(stock)] = stock
    flash[device.SLOTS[1] : device.SLOTS[1] + 0x3000] = b'\x5a' * 0x3000
    return bytes(flash), stock


class Flash(device.Device):
    """In-memory device: esptool reads, writes and digests act on a 16 MiB bytearray."""

    def __init__(self, memory, identity=IDENTITY):
        self.memory, self.identity = bytearray(memory), identity
        self.port, self.count = '/dev/fake', 0
        self.writes, self.reboots, self.connects = [], 0, 0

    def run(self, *args, **kwargs):
        raise AssertionError('tests never run esptool')

    def connect(self, strict=True):
        self.connects += 1
        data = bytes(self.memory[0x8000:0x10000])
        if strict:
            device.partitions(data)
            device.records(data)
        return data, self.identity

    def read(self, address, size, name):
        data = bytes(self.memory[address : address + size])
        (device.LOCAL / name).write_bytes(data)
        return data, ''

    def write(self, address, path):
        data = path.read_bytes()
        self.writes.append((address, len(data)))
        self.memory[address : address + len(data)] = data

    def verify(self, address, path):
        data = path.read_bytes()
        device.require(self.memory[address : address + len(data)] == data, 'Device digest mismatch')

    def reboot(self):
        self.reboots += 1

    def selected(self):
        return device.selected_slot(bytes(self.memory[0x8000:0x10000]))

    def owned(self):
        return bytes(self.memory[device.OWNED[0] : device.OWNED[0] + device.OWNED[1]])


class Workspace(unittest.TestCase):
    """A private .local with a verified backup and an in-memory device that boots what is selected."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.local = self.root / '.local/device'
        self.local.mkdir(parents=True)
        self.backup, self.stock = stock_flash()
        (self.local / 'flash.bin').write_bytes(self.backup)
        (self.local / 'stock.bin').write_bytes(self.stock)
        self.manifest = {
            'identity': IDENTITY,
            'flash_sha256': device.sha(self.backup),
            'stock_sha256': device.sha(self.stock),
            'stock_length': len(self.stock),
            'bootloader_region_sha256': device.sha(self.backup[:0x8000]),
        }
        (self.local / 'manifest.json').write_text(json.dumps(self.manifest))
        self.fake = Flash(self.backup)
        # Cycling data recorded since the backup: rides and settings at the end of slot B.
        self.fake.memory[device.OWNED[0] : device.OWNED[0] + 0x2000] = b'\x11' * 0x2000
        settings = device.SLOTS[1] + device.APP_SIZE + device.RIDE_SIZE
        self.fake.memory[settings : settings + 0x1000] = b'\x22' * 0x1000
        self.rides = self.fake.owned()
        self.booting = lambda slot: {'slot': device.NAMES[slot], 'commit': 'abc', 'cycling': 'true'}
        self.consoles = 0

        self.protocols = []

        def console(resolve, seconds, log, maintenance=False):
            self.consoles += 1
            self.protocols.append(maintenance)
            return self.booting(self.fake.selected())

        for target, value in (
            ('ROOT', self.root),
            ('LOCAL', self.local),
            ('Device', lambda port: self.fake),
            ('console_info', console),
        ):
            patcher = patch.object(device, target, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def candidate(self, data):
        (self.root / '.local/cycling.bin').write_bytes(data)
        return data

    def main(self, *args):
        output = io.StringIO()
        with patch('sys.argv', ['firmware', *args, '--port', '/dev/fake']), contextlib.redirect_stdout(output):
            device.main()
        return output.getvalue()

    def slot(self, slot, length):
        return bytes(self.fake.memory[device.SLOTS[slot] : device.SLOTS[slot] + length])


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
        self.assertEqual(device.image_span(good + b'\xff' * 32), len(good))
        self.assertIsNone(device.image_span(good[:24]))

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

    def test_only_cycling_images_are_candidates(self):
        device.validate_cycling(image())
        self.assertTrue(device.is_cycling(image()))
        self.assertFalse(device.is_cycling(stock_image()))
        with self.assertRaisesRegex(RuntimeError, 'not cycling firmware'):
            device.validate_cycling(stock_image())

    def test_application_cap_reserves_ride_region_and_settings_tail_in_both_slots(self):
        self.assertEqual(device.APP_SIZE, 0x638000)
        self.assertEqual(device.SLOTS[1] + device.APP_SIZE, 0xD98000)
        self.assertEqual(device.OWNED, (0xD98000, device.RIDE_SIZE + device.SETTINGS_SIZE))
        self.assertEqual(device.SLOTS[1] + device.APP_SIZE + device.RIDE_SIZE, 0xE98000)
        self.assertEqual(device.SLOTS[1] + device.SLOT_SIZE, 0xE9A000)
        self.assertLessEqual(device.SLOTS[0] + device.APP_SIZE, device.SLOTS[1])
        with patch.object(device, 'image_length', return_value=device.APP_SIZE), patch.object(device, 'is_cycling'):
            device.validate_cycling(bytes(device.APP_SIZE))
        with patch.object(device, 'image_length', return_value=device.APP_SIZE + 1), patch.object(device, 'is_cycling'):
            with self.assertRaises(RuntimeError):
                device.validate_cycling(bytes(device.APP_SIZE + 1))

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

    def test_highest_valid_sequence_selects_the_slot(self):
        self.assertEqual(device.selected_slot(metadata()), 0)
        self.assertEqual(device.selected_slot(metadata(((0x5000, 3), (0x6000, 4)))), 1)

    def test_selection_changes_only_one_sector_and_alternates(self):
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
                self.assertEqual(fake.select(fake.memory, target), fake.memory)
                self.assertEqual(device.selected_slot(fake.memory), target)
                self.assertEqual(fake.memory[:0x5000], original[:0x5000])
                self.assertEqual(fake.memory[0x7000:], original[0x7000:])
        self.assertEqual(fake.reboots, 5)
        self.assertTrue(all(address in (0xD000, 0xE000) for address in fake.writes))


class RestorePlanTests(unittest.TestCase):
    def test_identical_flash_needs_no_writes(self):
        backup, _ = stock_flash()
        self.assertEqual(device.restore_plan(backup, backup), [])

    def test_plan_reproduces_backup_with_application_first_and_bootloader_last(self):
        backup, _ = stock_flash()
        current = bytearray(backup)
        changes = [0x0, 0x1000, 0xD000, 0x20000, 0x23000, 0x40000, 0xD98000, 0xE99000]
        for offset in changes:
            current[offset] ^= 0xFF
        plan = device.restore_plan(bytes(current), backup)
        application = [(0x20000, 0x4000), (0x40000, 0x1000), (0xD98000, 0x1000), (0xE99000, 0x1000)]
        self.assertEqual(plan, [*application, (0xD000, 0x1000), (0x0, 0x2000)])
        for address, length in plan:
            current[address : address + length] = backup[address : address + length]
        self.assertEqual(bytes(current), backup)

    def test_boot_zone_never_rewrites_identical_sectors(self):
        backup, _ = stock_flash()
        current = bytearray(backup)
        current[0x0] ^= 1
        current[0x8000] ^= 1
        plan = device.restore_plan(bytes(current), backup)
        self.assertEqual(plan, [(0x8000, 0x1000), (0x0, 0x1000)])

    def test_partial_images_are_refused(self):
        with self.assertRaises(RuntimeError):
            device.restore_plan(b'', b'')


class VerifiedBackupTests(Workspace):
    def test_consistent_backup_is_accepted(self):
        manifest, backup = device.verified_backup()
        self.assertEqual(manifest['identity'], IDENTITY)
        self.assertEqual(backup, self.backup)

    def test_each_inconsistency_refuses(self):
        def flash(change):
            data = bytearray(self.backup)
            change(data)
            (self.local / 'flash.bin').write_bytes(data)

        def manifest(key, value):
            (self.local / 'manifest.json').write_text(json.dumps(self.manifest | {key: value}))

        stocked = bytearray(self.stock)
        stocked[-1] ^= 1
        cases = {
            'flash.bin hash': lambda: flash(lambda data: data.__setitem__(5, 0)),
            'stock.bin hash': lambda: (self.local / 'stock.bin').write_bytes(bytes(stocked)),
            'not the backup slot A': lambda: (
                (self.local / 'stock.bin').write_bytes(image()),
                manifest('stock_sha256', device.sha(image())),
            ),
            'bootloader region': lambda: manifest('bootloader_region_sha256', '0' * 64),
        }
        for message, damage in cases.items():
            with self.subTest(message=message):
                (self.local / 'flash.bin').write_bytes(self.backup)
                (self.local / 'stock.bin').write_bytes(self.stock)
                (self.local / 'manifest.json').write_text(json.dumps(self.manifest))
                damage()
                with self.assertRaisesRegex(RuntimeError, message):
                    device.verified_backup()

    def test_missing_backup_refuses(self):
        (self.local / 'manifest.json').unlink()
        with self.assertRaisesRegex(RuntimeError, 'No private backup'):
            device.verified_backup()


class BaselineTests(Workspace):
    """Device.baseline must prove device, backup and protected boot regions before any write."""

    def test_matching_device_passes_without_requiring_stock_in_slot_a(self):
        self.fake.memory[device.SLOTS[0] : device.SLOTS[0] + 0x1000] = b'\0' * 0x1000
        self.fake.baseline(IDENTITY, metadata())

    def test_missing_backup_manifest_refuses_before_device_access(self):
        (self.local / 'manifest.json').unlink()
        with self.assertRaisesRegex(RuntimeError, 'Run mise run backup'):
            self.fake.baseline(IDENTITY, metadata())

    def test_each_mismatch_refuses(self):
        cases = {
            'does not match the backup': lambda: self.fake.baseline('11:22:33:44:55:66', metadata()),
            'Backup integrity': lambda: (
                (self.local / 'flash.bin').write_bytes(b'x' + self.backup[1:]),
                self.fake.baseline(IDENTITY, metadata()),
            ),
            'Partition table changed': lambda: self.fake.baseline(IDENTITY, b'\0' * 0x8000),
            'Bootloader changed': lambda: (
                self.fake.memory.__setitem__(0, 0xAA),
                self.fake.baseline(IDENTITY, metadata()),
            ),
        }
        for message, run in cases.items():
            with self.subTest(message=message):
                (self.local / 'flash.bin').write_bytes(self.backup)
                self.fake.memory[:0x8000] = self.backup[:0x8000]
                with self.assertRaisesRegex(RuntimeError, message):
                    run()


class AbFlashTests(Workspace):
    def test_flash_alternates_slots_and_preserves_rides_settings_and_boot_metadata(self):
        first = self.candidate(image(marker=1))
        output = self.main('flash')
        self.assertEqual(self.fake.selected(), 1)
        self.assertEqual(self.slot(1, len(first)), first)
        self.assertEqual(self.slot(0, len(self.stock)), self.stock)
        self.assertIn('Slot A (stock) is selected; writing slot B', output)
        self.assertIn('Running slot B per INFO', output)

        second = self.candidate(image(marker=2))
        output = self.main('flash')
        self.assertEqual(self.fake.selected(), 0)
        self.assertEqual(self.slot(0, len(second)), second)
        self.assertEqual(self.slot(1, len(first)), first)
        self.assertIn('writing slot A (held stock; stock stays in the verified backup)', output)

        third = self.candidate(image(marker=3))
        self.main('flash')
        self.assertEqual(self.fake.selected(), 1)
        self.assertEqual(self.slot(1, len(third)), third)
        self.assertEqual(self.slot(0, len(second)), second)

        self.assertEqual(self.fake.owned(), self.rides)
        self.assertEqual(bytes(self.fake.memory[:0xD000]), self.backup[:0xD000])
        self.assertEqual(bytes(self.fake.memory[0xF000 : device.SLOTS[0]]), self.backup[0xF000 : device.SLOTS[0]])
        for address, length in self.fake.writes:
            end = address + length
            self.assertTrue(end <= device.OWNED[0] or address >= device.OWNED[0] + device.OWNED[1])
            self.assertTrue(address >= device.SLOTS[0] or address in (0xD000, 0xE000))

    def test_invalid_missing_or_foreign_image_never_connects(self):
        bad = bytearray(image())
        bad[-1] ^= 1
        for data in (bytes(bad), stock_image()):
            self.candidate(data)
            with self.assertRaises(RuntimeError):
                self.main('flash')
        (self.root / '.local/cycling.bin').unlink()
        with self.assertRaises(FileNotFoundError):
            self.main('flash')
        self.assertEqual(self.fake.connects, 0)

    def test_silent_console_switches_back_to_previous_cycling_slot(self):
        old = self.candidate(image(marker=1))
        self.main('flash')
        new = self.candidate(image(marker=2))
        self.booting = lambda slot: None if slot == 0 else {'slot': 'B'}
        with self.assertRaisesRegex(RuntimeError, 'Slot A did not answer INFO.*Reselected slot B; slot B answers'):
            self.main('flash')
        self.assertEqual(self.fake.selected(), 1)
        self.assertEqual(self.slot(1, len(old)), old)
        self.assertEqual(self.slot(0, len(new)), new)
        self.assertEqual(self.fake.owned(), self.rides)

    def test_bootloader_fallback_is_a_failed_boot(self):
        self.candidate(image())
        self.booting = lambda slot: {'slot': 'A'}
        with self.assertRaisesRegex(RuntimeError, 'INFO reports slot A.*Reselected slot A; it holds stock'):
            self.main('flash')
        self.assertEqual(self.fake.selected(), 0)
        self.assertEqual(self.consoles, 1)

    def test_older_firmware_without_slot_field_counts_as_booted(self):
        self.candidate(image())
        self.booting = lambda slot: {'commit': 'abc'}
        self.assertIn('Running slot B per OTA selection', self.main('flash'))

    def test_large_image_is_read_back_in_steps(self):
        large = image(extra=0x180000)
        self.fake.memory[device.SLOTS[0] : device.SLOTS[0] + len(large)] = large
        reads = []
        original = self.fake.read
        self.fake.read = lambda address, size, name: (reads.append(size), original(address, size, name))[1]
        self.assertEqual(self.fake.read_image(0), large)
        self.assertEqual(reads, [0x100000, 0x200000])


class ReviewFixTests(Workspace):
    def test_maintenance_build_is_probed_with_the_maintenance_protocol(self):
        maintenance = image(version=b'0.1.0+maintenance')
        self.assertTrue(device.speaks_maintenance(maintenance))
        self.assertFalse(device.speaks_maintenance(image()))
        self.assertFalse(device.speaks_maintenance(stock_image()))
        self.candidate(maintenance)
        self.main('flash')
        self.assertEqual(self.fake.selected(), 1)
        self.assertEqual(self.protocols, [True])
        # The stored image decides, never a leftover environment flag.
        self.candidate(image())
        with patch.dict(os.environ, {'CYCLING_BULK_MAINTENANCE': '1'}):
            self.main('flash')
        self.assertEqual(self.protocols, [True, False])

    def test_rollback_probes_the_protocol_of_the_target_image(self):
        self.candidate(image(version=b'0.1.0+maintenance'))
        self.main('flash')
        self.candidate(image())
        self.main('flash')
        self.protocols.clear()
        with patch.dict(os.environ, {}, clear=False) as environment:
            environment.pop('CYCLING_BULK_MAINTENANCE', None)
            self.main('rollback')
            self.assertEqual((self.fake.selected(), self.protocols), (1, [True]))
            self.main('rollback')
            self.assertEqual((self.fake.selected(), self.protocols), (0, [True, False]))

    def test_switch_back_probes_the_previous_image_with_its_own_protocol(self):
        self.candidate(image(version=b'0.1.0+maintenance'))
        self.main('flash')
        self.candidate(image())
        self.protocols.clear()
        self.booting = lambda slot: None if slot == 0 else {'protocol': 'maintenance'}
        with self.assertRaisesRegex(RuntimeError, 'Reselected slot B; slot B answers'):
            self.main('flash')
        self.assertEqual(self.protocols, [False, True])

    def test_backup_refuses_to_record_cycling_in_slot_a_as_stock(self):
        self.candidate(image())
        self.main('flash')
        self.main('flash')
        for name in ('manifest.json', 'flash.bin', 'stock.bin'):
            (self.local / name).unlink()
        # Slot A now holds cycling; pretend it is selected so only the image check can refuse.
        self.fake.memory[0x8000:0x10000] = metadata()
        with self.assertRaisesRegex(
            RuntimeError, 'Slot A holds cycling firmware, so it cannot become the stock baseline'
        ):
            self.main('backup')
        self.assertFalse((self.local / 'manifest.json').exists())
        self.assertFalse((self.local / 'stock.bin').exists())
        self.assertFalse((self.local / 'flash.bin').exists())

    def test_backup_of_stock_slot_a_still_works(self):
        for name in ('manifest.json', 'flash.bin', 'stock.bin'):
            (self.local / name).unlink()
        with patch.object(Flash, 'reboot'):
            self.main('backup')
        manifest = json.loads((self.local / 'manifest.json').read_text())
        self.assertEqual((self.local / 'stock.bin').read_bytes(), self.stock)
        self.assertEqual(manifest['stock_sha256'], device.sha(self.stock))

    def test_reset_failure_after_selection_reselects_previous_slot_from_fresh_metadata(self):
        self.candidate(image(marker=1))
        self.main('flash')
        self.candidate(image(marker=2))
        reboot, failures = self.fake.reboot, []

        def download_mode_once():
            if not failures:
                failures.append(self.fake.selected())
                raise RuntimeError('Device is still in ROM download mode')
            reboot()

        self.fake.reboot = download_mode_once
        connects = self.fake.connects
        with self.assertRaisesRegex(RuntimeError, 'Slot A failed after selection.*download mode.*Reselected slot B'):
            self.main('flash')
        self.assertEqual(failures, [0])
        self.assertEqual(self.fake.selected(), 1)
        self.assertEqual(self.fake.connects, connects + 2)
        self.assertEqual(self.consoles, 2)

    def test_failed_switch_back_names_the_recovery_commands(self):
        self.candidate(image())
        self.booting = lambda slot: None
        connect = self.fake.connect
        calls = []

        def once(strict=True):
            calls.append(strict)
            if len(calls) > 1:
                raise RuntimeError('esptool failed')
            return connect(strict)

        self.fake.connect = once
        with self.assertRaisesRegex(RuntimeError, 'switching back to slot A failed: esptool failed.*mise run rollback'):
            self.main('flash')

    def test_tty_configuration_failure_counts_as_attachment_failure(self):
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.object(device, 'open_no_reset', side_effect=termios.error(5, 'Input/output error')) as opened,
            patch.object(device.time, 'sleep'),
        ):
            self.assertIsNone(REAL_CONSOLE_INFO(lambda: '/dev/gone', 0.05, Path(temp) / 'console.log'))
        self.assertGreaterEqual(opened.call_count, 1)

    def test_restore_owned_puts_a_restore_snapshot_back_under_cycling(self):
        self.main('stock', '--confirm-restore')
        (snapshot,) = self.local.glob('owned-pre-restore-*.bin')
        self.assertNotEqual(self.fake.owned(), self.rides)
        with self.assertRaisesRegex(RuntimeError, 'does not hold cycling firmware'):
            self.main('restore-owned', str(snapshot))
        self.candidate(image())
        self.main('flash')
        newer = self.fake.owned()
        writes = len(self.fake.writes)
        output = self.main('restore-owned', str(snapshot))
        self.assertEqual(self.fake.owned(), self.rides)
        self.assertEqual(self.fake.writes[writes:], [device.OWNED])
        (before,) = self.local.glob('owned-before-restore-*.bin')
        self.assertEqual(before.read_bytes(), newer)
        self.assertIn('Rides and settings restored', output)

    def test_restore_owned_refuses_foreign_or_wrong_size_snapshots(self):
        outside = self.root / 'owned.bin'
        outside.write_bytes(self.rides)
        with self.assertRaisesRegex(RuntimeError, 'Snapshot must be a file under'):
            self.main('restore-owned', str(outside))
        short = self.local / 'short.bin'
        short.write_bytes(b'\0' * 16)
        self.candidate(image())
        self.main('flash')
        with self.assertRaisesRegex(RuntimeError, 'exactly'):
            self.main('restore-owned', str(short))
        with self.assertRaisesRegex(RuntimeError, 'needs a snapshot'):
            self.main('restore-owned')


class CrashJournalTests(Workspace):
    def test_a_written_crash_journal_never_blocks_or_meets_flash_rollback_and_stock(self):
        journal = b'CRJ1' + bytes(range(256)) * 31 + b'\xa5' * 0x1000
        self.fake.memory[0x1E000:0x20000] = journal[:0x2000]
        self.candidate(image(marker=1))
        self.main('flash')
        self.candidate(image(marker=2))
        self.main('flash')
        self.main('rollback')
        self.assertEqual(bytes(self.fake.memory[0x1E000:0x20000]), journal[:0x2000])
        for address, length in self.fake.writes:
            self.assertTrue(address + length <= 0x1E000 or address >= 0x20000)
        self.main('stock', '--confirm-restore')
        self.assertEqual(bytes(self.fake.memory), self.backup)


class RollbackTests(Workspace):
    def test_rollback_returns_to_the_other_cycling_image_without_writing_slots(self):
        old = self.candidate(image(marker=1))
        self.main('flash')
        new = self.candidate(image(marker=2))
        self.main('flash')
        writes = len(self.fake.writes)
        output = self.main('rollback')
        self.assertEqual(self.fake.selected(), 1)
        self.assertIn('Running slot B', output)
        self.assertEqual(self.slot(1, len(old)), old)
        self.assertEqual(self.slot(0, len(new)), new)
        self.assertTrue(all(address in (0xD000, 0xE000) for address, _ in self.fake.writes[writes:]))
        self.assertEqual(self.fake.owned(), self.rides)

    def test_rollback_refuses_when_the_other_slot_holds_stock(self):
        self.candidate(image())
        self.main('flash')
        writes = len(self.fake.writes)
        with self.assertRaisesRegex(RuntimeError, 'Slot A does not hold a valid cycling image'):
            self.main('rollback')
        self.assertEqual(self.fake.selected(), 1)
        self.assertEqual(len(self.fake.writes), writes)


class StockTests(Workspace):
    def test_full_restore_rewrites_differing_sectors_saves_cycling_data_and_resets(self):
        self.candidate(image(marker=1))
        self.main('flash')
        self.candidate(image(marker=2))
        self.main('flash')
        reboots = self.fake.reboots
        output = self.main('stock', '--confirm-restore')
        self.assertEqual(bytes(self.fake.memory), self.backup)
        self.assertEqual(self.fake.reboots, reboots + 1)
        snapshots = list(self.local.glob('owned-pre-restore-*.bin'))
        self.assertEqual(len(snapshots), 1)
        self.assertEqual(snapshots[0].read_bytes(), self.rides)
        self.assertIn(str(snapshots[0]), output)
        self.assertFalse((self.local / 'restore-before.bin').exists())
        self.assertFalse((self.local / 'restore-after.bin').exists())
        # Bootloader sectors were identical, so they were never rewritten.
        self.assertTrue(all(address >= 0x8000 for address, _ in self.fake.writes))

    def test_restore_refuses_a_mismatched_backup_before_device_access(self):
        (self.local / 'flash.bin').write_bytes(b'\x01' + self.backup[1:])
        with self.assertRaisesRegex(RuntimeError, 'hash mismatch'):
            self.main('stock', '--confirm-restore')
        self.assertEqual(self.fake.connects, 0)

    def test_restore_refuses_another_device(self):
        self.fake.identity = '11:22:33:44:55:66'
        with self.assertRaisesRegex(RuntimeError, 'does not match the backup'):
            self.main('stock', '--confirm-restore')
        self.assertEqual(self.fake.writes, [])

    def test_restore_readback_mismatch_fails_without_reset(self):
        write = self.fake.write
        self.fake.memory[device.SLOTS[1]] ^= 1
        self.fake.write = lambda address, path: (write(address, path), self.fake.memory.__setitem__(address, 0))
        with self.assertRaisesRegex(RuntimeError, r'sectors differ from the backup, first at 0x760000'):
            self.main('stock', '--confirm-restore')
        self.assertEqual(self.fake.reboots, 0)

    def test_restore_requires_confirmation(self):
        with self.assertRaisesRegex(RuntimeError, 'pass --confirm-restore'):
            device.confirm_restore(False, stdin=io.StringIO('RESTORE\n'))

        class Terminal(io.StringIO):
            def isatty(self):
                return True

        with contextlib.redirect_stdout(io.StringIO()):
            device.confirm_restore(False, stdin=Terminal('RESTORE\n'))
            with self.assertRaisesRegex(RuntimeError, 'cancelled'):
                device.confirm_restore(False, stdin=Terminal('yes\n'))
        device.confirm_restore(True, stdin=io.StringIO())
        with patch('sys.stdin', io.StringIO()), self.assertRaisesRegex(RuntimeError, 'pass --confirm-restore'):
            self.main('stock')
        self.assertEqual(self.fake.connects, 0)

    def test_stock_select_boots_slot_a_only_while_it_holds_stock(self):
        self.candidate(image())
        self.main('flash')
        self.main('stock-select')
        self.assertEqual(self.fake.selected(), 0)
        self.main('flash')
        self.assertEqual(self.fake.selected(), 1)
        self.main('flash')
        with self.assertRaisesRegex(RuntimeError, 'Slot A no longer holds stock'):
            self.main('stock-select')
        self.assertEqual(self.fake.selected(), 0)

    def test_backup_never_checks_baseline_or_writes(self):
        backup = Mock()
        with patch.object(Flash, 'backup', backup), patch.object(Flash, 'baseline') as baseline:
            self.main('backup')
        backup.assert_called_once()
        baseline.assert_not_called()
        self.assertEqual(self.fake.writes, [])


class ConsoleTests(unittest.TestCase):
    def test_console_info_returns_fields_of_the_first_ok_reply(self):
        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        port = os.ttyname(slave)

        def firmware():
            line = b''
            while not line.endswith(b'\n'):
                line += os.read(master, 256)
            request_id = int(line.split()[1])
            reply = {'type': 'reply', 'id': request_id, 'status': 'OK', 'data': 'board=c606 slot=B commit=abc'}
            os.write(master, json.dumps(reply).encode() + b'\n')

        thread = threading.Thread(target=firmware)
        thread.start()
        with tempfile.TemporaryDirectory() as temp:
            info = device.console_info(lambda: port, 5, Path(temp) / 'console.log')
        thread.join()
        os.close(slave)
        self.assertEqual(info['slot'], 'B')

    def test_maintenance_probe_reads_the_binary_info_frame(self):
        from cycling_devtools.device import mmc_maintenance as mmc

        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        port = os.ttyname(slave)

        def firmware():
            request = b''
            while len(request) < 32:
                request += os.read(master, 32 - len(request))
            sequence = mmc.HEADER.unpack(request)[2]
            data = struct.pack('<QII', 1000, 512, 7)
            head = mmc.HEADER.pack(b'C6RP', sequence, 0, 0, 0, 2, mmc.crc(data), 0)
            os.write(master, b'boot text\r\n' + head[:28] + struct.pack('<I', mmc.crc(head[:28])) + data)

        thread = threading.Thread(target=firmware)
        thread.start()
        with tempfile.TemporaryDirectory() as temp:
            info = device.console_info(lambda: port, 5, Path(temp) / 'console.log', maintenance=True)
        thread.join()
        os.close(slave)
        self.assertEqual(info['protocol'], 'maintenance')
        self.assertEqual(info['total_sectors'], 1000)

    def test_console_info_gives_up_after_its_deadline(self):
        def absent():
            raise OSError('no port')

        with tempfile.TemporaryDirectory() as temp, patch.object(device.time, 'sleep'):
            self.assertIsNone(device.console_info(absent, 0.05, Path(temp) / 'console.log'))


class SharedUsbLockTests(unittest.TestCase):
    def test_firmware_cannot_start_while_terminal_holds_existing_lock(self):
        from cycling_devtools.terminal import usb

        with tempfile.TemporaryDirectory() as directory, patch.object(usb, 'DEVICE_LOCAL', Path(directory) / '.local'):
            with usb.acquire_lock(), patch.object(device, 'main') as operation:
                with self.assertRaisesRegex(RuntimeError, 'Another cycling device task'):
                    device.run_locked()
                operation.assert_not_called()
            self.assertTrue((Path(directory) / '.local/usb.lock').is_file())

    def test_firmware_excludes_terminal_mmc_logs_and_releases_after_failure(self):
        from cycling_devtools.device import mmc_maintenance
        from cycling_devtools.terminal import logs, usb

        with tempfile.TemporaryDirectory() as directory, patch.object(usb, 'DEVICE_LOCAL', Path(directory) / '.local'):

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
