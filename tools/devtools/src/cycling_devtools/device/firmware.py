"""C606 backup, A/B cycling slots, rollback and full stock restore.

Stock lives in the verified private backup, not on the device: cycling may use
both application slots, and `stock` restores the complete 16 MiB backup. Writes
never touch eFuses, and flashes never touch the settings and ride reservations
at the end of slot B.
"""

import argparse
import hashlib
import json
import os
import re
import secrets
import struct
import subprocess
import sys
import termios
import time
import zlib
from pathlib import Path

from cycling_devtools.device import mmc_maintenance
from cycling_devtools.device.port import add_port_argument, select_port, selected_port
from cycling_devtools.terminal.usb import UsbConnection, acquire_lock, open_no_reset, reply_fields
from cycling_devtools.workspace import DEVICE_LOCAL, ROOT

LOCAL = DEVICE_LOCAL / 'device'
FLASH_SIZE = 0x1000000
SECTOR = 0x1000
SLOT_SIZE = 0x73A000
SETTINGS_SIZE = 0x2000
RIDE_SIZE = 0x100000
# Slot B ends with the ride journal and then settings, so an image there must
# stop before them. Slot A has no reservation; it uses the same cap so either
# slot can hold any image.
APP_SIZE = SLOT_SIZE - SETTINGS_SIZE - RIDE_SIZE
SLOTS = (0x20000, 0x760000)
NAMES = 'AB'
OWNED = (SLOTS[1] + APP_SIZE, RIDE_SIZE + SETTINGS_SIZE)
# Bootloader, partition table, NVS, OTA selection, PHY data and coredump.
BOOT_ZONE = SLOTS[0]
# Matches the harness reconnect bound for a restarted console.
CONSOLE_SECONDS = 25
APP_DESC_MAGIC = 0xABCD5432
PROJECT = b'c606-firmware'
# Descriptor version suffix of bulk-maintenance builds, which speak only the binary maintenance protocol.
MAINTENANCE_MARK = b'+maintenance'


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def image_length(data):
    """Validate a stock-compatible ESP32-S3 application and return its extent."""
    require(len(data) >= 24 and data[0] == 0xE9, 'Invalid ESP image header')
    require(1 <= data[1] <= 16 and struct.unpack_from('<H', data, 12)[0] == 9, 'Image must target ESP32-S3')
    offset, checksum = 24, 0xEF
    for _ in range(data[1]):
        require(offset + 8 <= len(data), 'Truncated segment header')
        size = struct.unpack_from('<I', data, offset + 4)[0]
        offset += 8
        require(offset + size <= len(data), 'Truncated segment')
        for value in data[offset : offset + size]:
            checksum ^= value
        offset += size
    end = (offset + 16) & ~15
    require(end <= len(data) and data[end - 1] == checksum, 'ESP checksum mismatch')
    require(data[23] == 1, 'Expected appended SHA-256')
    require(data[end : end + 32] == hashlib.sha256(data[:end]).digest(), 'ESP SHA-256 mismatch')
    return end + 32


def image_span(data):
    """Bytes the image at the start of data occupies, or None until every segment header is present."""
    require(len(data) >= 24 and data[0] == 0xE9 and 1 <= data[1] <= 16, 'Invalid ESP image header')
    offset = 24
    for _ in range(data[1]):
        if offset + 8 > len(data):
            return None
        offset += 8 + struct.unpack_from('<I', data, offset + 4)[0]
    return ((offset + 16) & ~15) + 32


def is_cycling(header):
    """The application descriptor at the start of the first segment names this firmware."""
    return (
        len(header) >= 0x70
        and header[0] == 0xE9
        and struct.unpack_from('<I', header, 0x20)[0] == APP_DESC_MAGIC
        and header[0x50:0x70].rstrip(b'\0') == PROJECT
    )


def speaks_maintenance(header):
    """Whether a cycling image is the bulk-maintenance build, from its stored descriptor version."""
    return is_cycling(header) and header[0x30:0x50].rstrip(b'\0').endswith(MAINTENANCE_MARK)


def describe(header, stock):
    """What a slot holds, from its first sector: cycling, stock or other."""
    if is_cycling(header):
        return 'cycling'
    return 'stock' if header == stock[:SECTOR] else 'other'


def validate_cycling(data):
    require(image_length(data) == len(data), 'Image has bytes outside its validated image extent')
    require(len(data) <= APP_SIZE, 'Image overlaps the reserved slot-B ride/settings storage')
    require(is_cycling(data), 'Image is not cycling firmware (application descriptor mismatch)')


def partitions(metadata):
    expected = [
        ('nvs', 1, 2, 0x9000, 0x4000),
        ('otadata', 1, 0, 0xD000, 0x2000),
        ('phy_init', 1, 1, 0xF000, 0x1000),
        ('coredump', 1, 3, 0x10000, 0x10000),
        ('ota_0', 0, 0x10, SLOTS[0], SLOT_SIZE),
        ('ota_1', 0, 0x11, SLOTS[1], SLOT_SIZE),
    ]
    entries = []
    for offset in range(0, 0xC00, 32):
        entry = metadata[offset : offset + 32]
        magic = struct.unpack_from('<H', entry)[0]
        if magic == 0xEBEB:
            require(hashlib.md5(metadata[:offset]).digest() == entry[16:], 'Partition MD5 mismatch')
            break
        require(magic == 0x50AA, 'Unexpected partition table')
        kind, subtype, address, size, label, flags = struct.unpack('<BBII16sI', entry[2:])
        require(flags == 0, 'Encrypted or flagged partition is unsupported')
        entries.append((label.rstrip(b'\0').decode(), kind, subtype, address, size))
    # Stock labels can differ; layout and types must match exactly.
    require([e[1:] for e in entries] == [e[1:] for e in expected], 'Unsupported C606 partition layout')


def records(metadata):
    result = []
    for offset in (0x5000, 0x6000):
        record = metadata[offset : offset + 32]
        seq, state, crc = struct.unpack_from('<I20xII', record)
        valid = seq not in (0, 0xFFFFFFFF) and state not in (3, 4)
        valid &= zlib.crc32(record[:4], 0xFFFFFFFF) == crc
        if valid:
            result.append((seq, offset))
    require(result, 'No valid OTA selection record')
    return result


def selected_slot(metadata):
    """Slot the bootloader starts: the highest valid sequence selects (sequence - 1) % 2."""
    return (max(records(metadata))[0] - 1) % 2


def verified_backup(local=None):
    """Prove the private backup is complete and self-consistent before it may be written."""
    local = LOCAL if local is None else local
    require((local / 'manifest.json').exists(), 'No private backup; stock cannot be restored')
    manifest = json.loads((local / 'manifest.json').read_text())
    backup = (local / 'flash.bin').read_bytes()
    stock = (local / 'stock.bin').read_bytes()
    require(len(backup) == FLASH_SIZE and sha(backup) == manifest['flash_sha256'], 'Backup flash.bin hash mismatch')
    require(sha(stock) == manifest['stock_sha256'], 'Backup stock.bin hash mismatch')
    require(backup[SLOTS[0] : SLOTS[0] + len(stock)] == stock, 'stock.bin is not the backup slot A image')
    require(sha(backup[:0x8000]) == manifest['bootloader_region_sha256'], 'Backup bootloader region mismatch')
    partitions(backup[0x8000:0x10000])
    require(selected_slot(backup[0x8000:0x10000]) == 0, 'Backup does not select stock slot A')
    return manifest, backup


def sectors_differing(actual, expected):
    return [
        offset
        for offset in range(0, len(expected), SECTOR)
        if actual[offset : offset + SECTOR] != expected[offset : offset + SECTOR]
    ]


def restore_plan(current, backup, gap=0x10000):
    """Runs of differing sectors to write: application zone first, boot metadata last.

    Application runs merge across short identical gaps to bound esptool calls.
    Boot metadata stays sector-exact so identical bootloader sectors are never
    rewritten, and is written top-down, bootloader last.
    """
    require(len(current) == len(backup) == FLASH_SIZE, 'Restore needs complete 16 MiB images')
    application, boot = [], []
    for offset in sectors_differing(current, backup):
        runs, merge = (boot, 0) if offset < BOOT_ZONE else (application, gap)
        if runs and offset - (runs[-1][0] + runs[-1][1]) <= merge:
            runs[-1][1] = offset + SECTOR - runs[-1][0]
        else:
            runs.append([offset, SECTOR])
    return [tuple(run) for run in application] + [tuple(run) for run in reversed(boot)]


def console_info(resolve, seconds, log, maintenance=False):
    """Fields of the first readiness reply within seconds, else None; read-only, never resets.

    Ordinary builds answer the JSON terminal's INFO. The bulk-maintenance build
    speaks only the binary maintenance protocol, so it is asked for its INFO frame.
    """
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        fd = None
        try:
            port = resolve()
            fd = open_no_reset(port)
            # The device lock is already held by this process; drive the protocol on this fd directly.
            if maintenance:
                connection = mmc_maintenance.Connection(port)
                connection.fd = fd
                return {'protocol': 'maintenance', **connection.info()}
            connection = UsbConnection(port, log)
            connection.fd, connection.request_id = fd, secrets.randbelow(1 << 30) + 1
            with open(log, 'ab') as connection.log:
                reply = connection.terminal_command('INFO', timeout=min(3, max(0.1, deadline - time.monotonic())))
            if reply['status'] == 'OK':
                return reply_fields(reply['data'])
        except OSError, RuntimeError, TimeoutError, ValueError, termios.error:
            # Absent, re-enumerating, disconnected while configuring, or still
            # starting: an attachment failure. Ask again with a new request.
            pass
        finally:
            if fd is not None:
                os.close(fd)
        time.sleep(0.5)
    return None


class Device:
    def __init__(self, port):
        self.port = port
        self.count = 0
        LOCAL.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(LOCAL, 0o700)

    def run(self, *args, before='no-reset', after='no-reset'):
        self.count += 1
        command = [
            sys.executable,
            '-m',
            'esptool',
            '--chip',
            'esp32s3',
            '--port',
            self.port,
            '--before',
            before,
            '--after',
            after,
            *map(str, args),
        ]
        result = subprocess.run(command, capture_output=True, text=True)
        output = result.stdout + result.stderr
        (LOCAL / f'{time.time_ns()}-{self.count}.log').write_text(output)
        require(result.returncode == 0, f'esptool failed; inspect private logs in {LOCAL}')
        return output

    def read(self, address, size, name):
        path = LOCAL / name
        output = self.run('read-flash', hex(address), hex(size), path)
        data = path.read_bytes()
        require(len(data) == size, 'Short flash read')
        return data, output

    def connect(self, strict=True):
        """Enter download mode and identify the device; strict also requires the known boot layout."""
        output = self.run('--no-stub', 'get-security-info', before='usb-reset')
        require(
            'Secure Boot: Disabled' in output and 'Flash Encryption: Disabled' in output,
            'This procedure requires secure boot and flash encryption disabled',
        )
        metadata, output = self.read(0x8000, 0x8000, 'before.bin')
        mac = re.search(r'MAC:\s*([0-9a-f:]{17})', output, re.I)
        require(mac is not None, 'Could not establish device identity')
        if strict:
            partitions(metadata)
            records(metadata)
        return metadata, mac[1].lower()

    def verify(self, address, path):
        output = self.run('verify-flash', hex(address), path)
        require('Verification successful (digest matched).' in output, 'Device digest mismatch')

    def write(self, address, path):
        self.run(
            'write-flash', '--flash-mode', 'keep', '--flash-freq', 'keep', '--flash-size', 'keep', hex(address), path
        )

    def reboot(self):
        # RTS hard-reset left this C606 in ROM download mode. The watchdog
        # reset boots the selected app, and stock can then remove the USB port.
        self.run('get-security-info', after='watchdog-reset')
        path = LOCAL / f'{time.time_ns()}-boot.log'
        with path.open('wb') as output:
            try:
                monitor(self.port, 3, output=output)
            except OSError:
                print('USB boot capture unavailable after reset.')
        captured = path.read_bytes()
        require(
            b'waiting for download' not in captured and b'DOWNLOAD(USB/UART0)' not in captured,
            f'Device is still in ROM download mode; inspect private boot logs in {LOCAL}',
        )

    def backup(self, metadata, identity):
        require(selected_slot(metadata) == 0, 'Boot stock slot A before making the baseline backup')
        require(not (LOCAL / 'manifest.json').exists(), 'A backup already exists; it will not be overwritten')
        # Cycling may occupy slot A now, so a fresh host must never record it as the stock baseline.
        refusal = (
            'Slot A holds cycling firmware, so it cannot become the stock baseline; copy the existing '
            f'verified backup (flash.bin, stock.bin, manifest.json) into {LOCAL} instead'
        )
        require(not is_cycling(self.header(0)), refusal)
        print('Reading the full 16 MiB flash. This takes about two minutes.', flush=True)
        backup, _ = self.read(0, FLASH_SIZE, 'flash.bin')
        self.verify(0, LOCAL / 'flash.bin')
        stock = backup[SLOTS[0] : SLOTS[0] + SLOT_SIZE]
        stock = stock[: image_length(stock)]
        require(not is_cycling(stock), refusal)
        (LOCAL / 'stock.bin').write_bytes(stock)
        manifest = {
            'identity': identity,
            'flash_sha256': sha(backup),
            'stock_sha256': sha(stock),
            'stock_length': len(stock),
            'bootloader_region_sha256': sha(backup[:0x8000]),
        }
        (LOCAL / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        self.reboot()
        print('Full backup verified; stock watchdog reset sent. Confirm the screen.')

    def baseline(self, identity, metadata):
        """Before any slot write: same device, intact backup, unchanged partition table and bootloader."""
        require((LOCAL / 'manifest.json').exists(), 'Run mise run backup before flashing')
        manifest = json.loads((LOCAL / 'manifest.json').read_text())
        backup = (LOCAL / 'flash.bin').read_bytes()
        stock = (LOCAL / 'stock.bin').read_bytes()
        require(identity == manifest['identity'], 'Connected device does not match the backup')
        require(
            sha(backup) == manifest['flash_sha256'] and sha(stock) == manifest['stock_sha256'],
            'Backup integrity check failed',
        )
        require(metadata[:0x1000] == backup[0x8000:0x9000], 'Partition table changed since backup')
        boot, _ = self.read(0, 0x8000, 'bootloader-check.bin')
        require(sha(boot) == manifest['bootloader_region_sha256'], 'Bootloader changed since backup')

    def snapshot_owned(self, prefix='owned-before'):
        # A fresh read for this operation, never the original stock baseline.
        name = f'{prefix}-{time.time_ns()}.bin'
        self.read(*OWNED, name)
        return OWNED[0], LOCAL / name

    def header(self, slot):
        header, _ = self.read(SLOTS[slot], SECTOR, f'slot-{NAMES[slot].lower()}-header.bin')
        return header

    def contents(self, slot):
        """Describe what a slot holds from its first sector: cycling, stock or other."""
        return describe(self.header(slot), (LOCAL / 'stock.bin').read_bytes())

    def read_image(self, slot):
        """Read only as much of a slot as its image occupies, then require a valid cycling image."""
        size = 0x100000
        while True:
            data, _ = self.read(SLOTS[slot], size, f'slot-{NAMES[slot].lower()}.bin')
            try:
                span = image_span(data)
                if span is not None and span <= len(data):
                    validate_cycling(data[:span])
                    return data[:span]
                require(size < APP_SIZE and (span or 0) <= APP_SIZE, 'Image exceeds the slot cap')
            except RuntimeError as error:
                raise RuntimeError(f'Slot {NAMES[slot]} does not hold a valid cycling image: {error}') from None
            size = min(APP_SIZE, max(size * 2, -(-(span or 0) // SECTOR) * SECTOR))

    def select(self, metadata, slot):
        valid = records(metadata)
        latest, newest_offset = max(valid)
        require(latest < 0xFFFFFFFC, 'OTA sequence needs manual inspection before rollover')
        sequence = latest + 1
        if (sequence - 1) % 2 != slot:
            sequence += 1
        offset = 0x6000 if newest_offset == 0x5000 else 0x5000
        sector = bytearray(b'\xff' * 0x1000)
        struct.pack_into('<I', sector, 0, sequence)
        struct.pack_into('<I', sector, 28, zlib.crc32(sector[:4], 0xFFFFFFFF))
        path = LOCAL / 'selection.bin'
        path.write_bytes(sector)
        expected = bytearray(metadata)
        expected[offset : offset + 0x1000] = sector
        self.write(0x8000 + offset, path)
        actual, _ = self.read(0x8000, 0x8000, 'after.bin')
        require(actual == expected, 'Boot selection readback does not match the planned change')
        self.reboot()
        print(f'Slot {NAMES[slot]} selected; watchdog reset sent.', flush=True)
        return bytes(expected)

    def boot(
        self,
        metadata,
        slot,
        previous,
        resolve,
        seconds=CONSOLE_SECONDS,
        previous_console=True,
        maintenance=False,
        previous_maintenance=False,
    ):
        """Select a cycling slot and require its console; otherwise reselect the previous slot.

        maintenance and previous_maintenance pick each image's readiness protocol
        from its stored descriptor; previous_console is false when the previous slot holds stock.
        """
        previous_probe = previous_maintenance if previous_console else None
        try:
            self.select(metadata, slot)
        except RuntimeError as error:
            # The new selection may already be written (for example, the reset
            # left ROM download mode): recover from fresh metadata.
            self.switch_back(slot, previous, f'failed after selection ({error})', resolve, seconds, previous_probe)
        info = console_info(resolve, seconds, LOCAL / f'{time.time_ns()}-console.log', maintenance)
        reported = None if info is None else info.get('slot')
        if info is not None and reported in (None, NAMES[slot]):
            detail = ' '.join(
                f'{key}={info[key]}' for key in ('commit', 'cycling', 'harness', 'protocol') if key in info
            )
            source = 'INFO' if reported else 'OTA selection (this firmware does not report its slot)'
            print(f'Running slot {NAMES[slot]} per {source}; {detail}')
            return info
        problem = (
            f'did not answer INFO within {seconds} s'
            if info is None
            else f'was selected but INFO reports slot {reported}'
        )
        self.switch_back(slot, previous, problem, resolve, seconds, previous_probe)

    def switch_back(self, slot, previous, problem, resolve, seconds, previous_probe):
        """Reselect the previous slot from freshly read metadata, then fail with what happened."""
        print(f'Slot {NAMES[slot]} {problem}; switching back to slot {NAMES[previous]}.', flush=True)
        try:
            self.port = resolve()
            current, _ = self.connect()
            self.select(current, previous)
        except (RuntimeError, OSError, ValueError) as error:
            raise RuntimeError(
                f'Slot {NAMES[slot]} {problem}, and switching back to slot {NAMES[previous]} failed: {error}. '
                f'Reconnect, then run mise run rollback or mise run stock. Logs: {LOCAL}'
            ) from None
        if previous_probe is not None:
            back = console_info(resolve, seconds, LOCAL / f'{time.time_ns()}-console.log', previous_probe)
            state = f'slot {back.get("slot", NAMES[previous])} answers INFO' if back else 'its console is silent too'
        else:
            state = 'it holds stock; confirm the screen'
        raise RuntimeError(f'Slot {NAMES[slot]} {problem}. Reselected slot {NAMES[previous]}; {state}. Logs: {LOCAL}')

    def restore_owned(self, metadata, snapshot):
        """Write a saved ride/settings snapshot back into the reservations under running cycling firmware."""
        data = Path(snapshot).read_bytes()
        require(len(data) == OWNED[1], f'Snapshot must be exactly {OWNED[1]:#x} bytes of rides and settings')
        require(
            self.contents(selected_slot(metadata)) == 'cycling',
            'The selected slot does not hold cycling firmware; flash cycling before restoring rides and settings',
        )
        _, before = self.snapshot_owned('owned-before-restore')
        print(f'Saved the current reservations to {before}; writing {snapshot}.', flush=True)
        self.write(OWNED[0], Path(snapshot))
        self.verify(OWNED[0], Path(snapshot))
        after, _ = self.read(*OWNED, 'owned-after-restore.bin')
        require(after == data, 'Ride/settings readback does not match the snapshot')
        (LOCAL / 'owned-after-restore.bin').unlink(missing_ok=True)
        self.reboot()
        print('Rides and settings restored and read back; watchdog reset sent so the firmware reloads them.')

    def restore(self, backup):
        """Write the verified backup over every differing sector, then read the whole flash back."""
        print('Reading the full 16 MiB flash before restore. This takes about two minutes.', flush=True)
        current, _ = self.read(0, FLASH_SIZE, 'restore-before.bin')
        snapshot = LOCAL / f'owned-pre-restore-{time.time_ns()}.bin'
        snapshot.write_bytes(current[OWNED[0] : OWNED[0] + OWNED[1]])
        print(
            f'Saved current ride journal (offset 0, {RIDE_SIZE:#x} bytes) and settings '
            f'(offset {RIDE_SIZE:#x}, {SETTINGS_SIZE:#x} bytes) from {OWNED[0]:#x} to {snapshot}',
            flush=True,
        )
        print(
            'The restore puts the backup bytes back into these reservations, so cycling firmware will not persist '
            'settings or rides there until they are restored. After flashing cycling again, run: '
            f'mise run restore-owned -- {snapshot}',
            flush=True,
        )
        plan = restore_plan(current, backup)
        print(f'Restoring {sum(length for _, length in plan) // SECTOR} differing sectors in {len(plan)} writes.')
        run = LOCAL / 'restore-run.bin'
        for address, length in plan:
            run.write_bytes(backup[address : address + length])
            self.write(address, run)
        print('Reading the full flash back for sector-wise verification.', flush=True)
        after, _ = self.read(0, FLASH_SIZE, 'restore-after.bin')
        bad = sectors_differing(after, backup)
        if bad:
            raise RuntimeError(f'{len(bad)} sectors differ from the backup, first at {bad[0]:#x}; rerun mise run stock')
        for name in ('restore-before.bin', 'restore-after.bin', 'restore-run.bin'):
            (LOCAL / name).unlink(missing_ok=True)
        self.reboot()
        print('Full stock restore verified; watchdog reset sent. Stock can remove USB: confirm the screen.')
        return snapshot

    def stock_select(self, metadata):
        """Light path: boot slot A only while it still holds the verified stock image."""
        try:
            self.verify(SLOTS[0], LOCAL / 'stock.bin')
        except RuntimeError:
            raise RuntimeError('Slot A no longer holds stock; use mise run stock for the full restore') from None
        self.select(metadata, 0)
        print('Stock can remove USB: confirm the screen.')


def monitor(port, seconds, output=None):
    import serial

    connection = serial.Serial()
    connection.port, connection.baudrate, connection.timeout = port, 115200, 0.2
    connection.dtr = connection.rts = False
    connection.open()
    output = sys.stdout.buffer if output is None else output
    deadline = time.monotonic() + seconds
    with connection:
        while time.monotonic() < deadline:
            data = connection.read(4096)
            output.write(data)
            output.flush()


def confirm_restore(confirmed, stdin=None):
    if confirmed:
        return
    stdin = sys.stdin if stdin is None else stdin
    require(stdin.isatty(), 'A full stock restore rewrites the whole 16 MiB flash; pass --confirm-restore')
    print('This rewrites the whole flash with the verified stock backup (eFuses untouched).')
    print(
        'Current cycling rides and settings are saved privately first. Type RESTORE to continue: ', end='', flush=True
    )
    require(stdin.readline().strip() == 'RESTORE', 'Restore cancelled')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        'action', choices=['backup', 'flash', 'rollback', 'stock', 'stock-select', 'restore-owned', 'monitor']
    )
    add_port_argument(parser)
    parser.add_argument('--seconds', type=int, default=15, help='monitor duration')
    parser.add_argument(
        '--boot-seconds', type=int, default=CONSOLE_SECONDS, help='console deadline before reselecting the old slot'
    )
    parser.add_argument('snapshot', nargs='?', type=Path, help='restore-owned: a saved owned-*.bin snapshot')
    parser.add_argument('--confirm-restore', action='store_true', help='stock: skip the interactive confirmation')
    args = parser.parse_args()
    port = selected_port(parser, args.port)

    def resolve():
        return select_port(args.port)

    if args.action == 'monitor':
        monitor(port, args.seconds)
        return
    # Everything checkable offline is checked before interrupting the running firmware.
    candidate = ROOT / '.local/cycling.bin'
    if args.action == 'flash':
        image = candidate.read_bytes()
        validate_cycling(image)
    if args.action == 'restore-owned':
        require(args.snapshot is not None, 'restore-owned needs a snapshot path')
        require(
            args.snapshot.resolve().is_relative_to(LOCAL.resolve()) and args.snapshot.is_file(),
            f'Snapshot must be a file under {LOCAL}',
        )
    if args.action == 'stock':
        confirm_restore(args.confirm_restore)
        manifest, backup = verified_backup()
    device = Device(port)
    if args.action == 'stock':
        _, identity = device.connect(strict=False)
        require(identity == manifest['identity'], 'Connected device does not match the backup')
        device.restore(backup)
        return
    metadata, identity = device.connect()
    if args.action == 'backup':
        device.backup(metadata, identity)
        return
    device.baseline(identity, metadata)
    if args.action == 'stock-select':
        device.stock_select(metadata)
        return
    if args.action == 'restore-owned':
        device.restore_owned(metadata, args.snapshot)
        info = console_info(resolve, args.boot_seconds, LOCAL / f'{time.time_ns()}-console.log')
        print(f'Console answers INFO on slot {info.get("slot", "?")}.' if info else 'Console did not answer INFO.')
        return
    running = selected_slot(metadata)
    other = 1 - running
    running_header = device.header(running)
    running_contents = describe(running_header, (LOCAL / 'stock.bin').read_bytes())
    if args.action == 'rollback':
        image = device.read_image(other)
        print(f'Slot {NAMES[running]} is selected; returning to the cycling image in slot {NAMES[other]}.', flush=True)
    else:
        target_contents = device.contents(other)
        note = '; stock stays in the verified backup' if target_contents == 'stock' else ''
        print(
            f'Slot {NAMES[running]} ({running_contents}) is selected; writing slot {NAMES[other]} '
            f'(held {target_contents}{note}). Rides and settings are preserved.',
            flush=True,
        )
        owned_address, owned_before = device.snapshot_owned()
        device.write(SLOTS[other], candidate)
        device.verify(SLOTS[other], candidate)
        device.verify(owned_address, owned_before)
    device.boot(
        metadata,
        other,
        running,
        resolve,
        args.boot_seconds,
        previous_console=running_contents == 'cycling',
        # Each image's descriptor, not the environment, says which protocol its console speaks.
        maintenance=speaks_maintenance(image),
        previous_maintenance=speaks_maintenance(running_header),
    )


def run_locked():
    """Serialize flashing and boot selection with all no-reset USB tools."""
    try:
        lock = acquire_lock()
    except BlockingIOError:
        raise RuntimeError('Another cycling device task is using USB; wait until it finishes') from None
    with lock:
        main()


if __name__ == '__main__':
    try:
        run_locked()
    except (RuntimeError, OSError) as error:
        sys.exit(str(error))
