"""Backup, A/B cycling slots, rollback and full stock restore, on the layout of the device manifest.

Stock lives in the verified private backup, not on the device: cycling may use
both application slots, and `stock` restores the complete flash backup. Writes
never touch eFuses, and flashes never touch the settings and ride reservations
at the end of slot B.
"""

import argparse
import hashlib
import json
import os
import secrets
import struct
import sys
import termios
import time
import zlib
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools import devices
from cycling_devtools.device import mmc_maintenance
from cycling_devtools.device.flasher import EsptoolFlasher, Flasher, monitor
from cycling_devtools.device.page_flasher import PageFlasher
from cycling_devtools.device.port import add_port_argument, select_port, selected_port
from cycling_devtools.terminal import time_sync
from cycling_devtools.terminal.usb import UsbConnection, acquire_lock, open_no_reset, reply_fields
from cycling_devtools.workspace import DEVICE_LOCAL, ROOT

LOCAL = DEVICE_LOCAL / 'device'
SECTOR = devices.ERASE_SECTOR
NAMES = 'AB'
# Matches the harness reconnect bound for a restarted console.
CONSOLE_SECONDS = 25
BOOT_CAPTURE_SECONDS = 3
APP_DESC_MAGIC = 0xABCD5432
# Descriptor version suffix of bulk-maintenance builds, which speak only the binary maintenance protocol.
MAINTENANCE_MARK = b'+maintenance'


@dataclass(frozen=True)
class Layout:
    name: str
    project: bytes
    flash_size: int
    slots: tuple[int, ...]
    slot_size: int
    app_size: int
    owned: tuple[int, int]
    partitions: tuple[devices.Partition, ...]
    metadata: tuple[int, int]
    otadata: int

    @property
    def boot_zone(self):
        return self.slots[0]

    @classmethod
    def of(cls, device):
        table, otadata = device.partition_table, device.partition('otadata')
        return cls(
            name=device.name,
            project=device.package.encode(),
            flash_size=device.flash_size,
            slots=tuple(slot.offset for slot in device.slots),
            slot_size=device.slots[0].size,
            app_size=device.image_cap,
            owned=device.owned_tail,
            partitions=device.partitions,
            metadata=(table, otadata.end - table),
            otadata=otadata.offset - table,
        )


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


def is_cycling(header, layout):
    """The application descriptor at the start of the first segment names this firmware."""
    return (
        len(header) >= 0x70
        and header[0] == 0xE9
        and struct.unpack_from('<I', header, 0x20)[0] == APP_DESC_MAGIC
        and header[0x50:0x70].rstrip(b'\0') == layout.project
    )


def speaks_maintenance(header, layout):
    """Whether a cycling image is the bulk-maintenance build, from its stored descriptor version."""
    return is_cycling(header, layout) and header[0x30:0x50].rstrip(b'\0').endswith(MAINTENANCE_MARK)


def describe(header, stock, layout):
    """What a slot holds, from its first sector: cycling, stock or other."""
    if is_cycling(header, layout):
        return 'cycling'
    return 'stock' if header == stock[:SECTOR] else 'other'


def validate_cycling(data, layout):
    require(image_length(data) == len(data), 'Image has bytes outside its validated image extent')
    require(len(data) <= layout.app_size, 'Image overlaps the reserved slot-B ride/settings storage')
    require(is_cycling(data, layout), 'Image is not cycling firmware (application descriptor mismatch)')


def partitions(metadata, layout):
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
    expected = [p.entry for p in layout.partitions]
    require([e[1:] for e in entries] == expected, f'Partition layout differs from devices/{layout.name}/device.toml')


def records(metadata, layout):
    result = []
    for offset in (layout.otadata, layout.otadata + SECTOR):
        record = metadata[offset : offset + 32]
        seq, state, crc = struct.unpack_from('<I20xII', record)
        valid = seq not in (0, 0xFFFFFFFF) and state not in (3, 4)
        valid &= zlib.crc32(record[:4], 0xFFFFFFFF) == crc
        if valid:
            result.append((seq, offset))
    require(result, 'No valid OTA selection record')
    return result


def selected_slot(metadata, layout):
    """Slot the bootloader starts: the highest valid sequence selects (sequence - 1) % 2."""
    return (max(records(metadata, layout))[0] - 1) % 2


def verified_backup(layout, local=None):
    """Prove the private backup is complete and self-consistent before it may be written."""
    local = LOCAL if local is None else local
    require((local / 'manifest.json').exists(), 'No private backup; stock cannot be restored')
    manifest = json.loads((local / 'manifest.json').read_text())
    backup = (local / 'flash.bin').read_bytes()
    stock = (local / 'stock.bin').read_bytes()
    require(
        len(backup) == layout.flash_size and sha(backup) == manifest['flash_sha256'], 'Backup flash.bin hash mismatch'
    )
    require(sha(stock) == manifest['stock_sha256'], 'Backup stock.bin hash mismatch')
    require(backup[layout.slots[0] : layout.slots[0] + len(stock)] == stock, 'stock.bin is not the backup slot A image')
    require(
        sha(backup[: layout.metadata[0]]) == manifest['bootloader_region_sha256'], 'Backup bootloader region mismatch'
    )
    metadata = backup[layout.metadata[0] : sum(layout.metadata)]
    partitions(metadata, layout)
    require(selected_slot(metadata, layout) == 0, 'Backup does not select stock slot A')
    return manifest, backup


def sectors_differing(actual, expected):
    return [
        offset
        for offset in range(0, len(expected), SECTOR)
        if actual[offset : offset + SECTOR] != expected[offset : offset + SECTOR]
    ]


def restore_plan(current, backup, layout, gap=0x10000):
    """Runs of differing sectors to write: application zone first, boot metadata last.

    Application runs merge across short identical gaps to bound esptool calls.
    Boot metadata stays sector-exact so identical bootloader sectors are never
    rewritten, and is written top-down, bootloader last.
    """
    require(len(current) == len(backup) == layout.flash_size, 'Restore needs complete 16 MiB images')
    application, boot = [], []
    for offset in sectors_differing(current, backup):
        runs, merge = (boot, 0) if offset < layout.boot_zone else (application, gap)
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


def set_clock(resolve, log):
    fd = None
    try:
        port = resolve()
        fd = open_no_reset(port)
        connection = UsbConnection(port, log)
        connection.fd, connection.request_id = fd, secrets.randbelow(1 << 30) + 1
        with open(log, 'ab') as connection.log:
            reply = time_sync.sync(connection)
        print(f'Clock set from this machine: {reply["data"]}', flush=True)
    except (OSError, RuntimeError, TimeoutError, ValueError, termios.error) as error:
        print(f'Clock not set ({error}); run cycling time-sync', flush=True)
    finally:
        if fd is not None:
            os.close(fd)


def flasher_for(port, resolve, log_dir, environ=os.environ, chip='esp32s3') -> Flasher:
    """The visitor's page only for the bridged port, so a second, local device in a bridged command stays local.

    `chip` reaches esptool only: the page detects the family itself.
    """
    bridged, door = environ.get('CYCLING_PORT'), environ.get('CYCLING_FLASHER')
    if bridged and door and port == bridged:
        return PageFlasher(Path(door), log_dir)
    return EsptoolFlasher(port, resolve, log_dir, chip)


class Device:
    def __init__(self, flasher: Flasher, layout: Layout):
        self.flasher, self.layout = flasher, layout
        LOCAL.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(LOCAL, 0o700)

    def read(self, address, size, name):
        path = LOCAL / name
        self.flasher.read(address, size, path)
        data = path.read_bytes()
        require(len(data) == size, 'Short flash read')
        return data

    def connect(self, strict=True):
        """Enter download mode and identify the device; strict also requires the known boot layout."""
        chip = self.flasher.enter()
        require(
            chip.accepts_unsigned is not None,
            f'This {chip.family} cannot report secure boot or flash encryption, so it is treated as not writable',
        )
        require(chip.accepts_unsigned, 'This procedure requires secure boot and flash encryption disabled')
        metadata = self.read(*self.layout.metadata, 'before.bin')
        if strict:
            partitions(metadata, self.layout)
            records(metadata, self.layout)
        return metadata, chip.identity

    def verify(self, address, path):
        require(self.flasher.verify(address, Path(path)), 'Device digest mismatch')

    def write(self, address, path):
        self.flasher.write(address, Path(path))

    def reboot(self):
        captured = self.flasher.reboot(BOOT_CAPTURE_SECONDS)
        if captured is None:
            print('USB boot capture unavailable after reset.')
            return
        (LOCAL / f'{time.time_ns()}-boot.log').write_bytes(captured)
        require(
            b'waiting for download' not in captured and b'DOWNLOAD(USB/UART0)' not in captured,
            f'Device is still in ROM download mode; inspect private boot logs in {LOCAL}',
        )

    def backup(self, metadata, identity):
        require(selected_slot(metadata, self.layout) == 0, 'Boot stock slot A before making the baseline backup')
        require(not (LOCAL / 'manifest.json').exists(), 'A backup already exists; it will not be overwritten')
        # Cycling may occupy slot A now, so a fresh host must never record it as the stock baseline.
        refusal = (
            'Slot A holds cycling firmware, so it cannot become the stock baseline; copy the existing '
            f'verified backup (flash.bin, stock.bin, manifest.json) into {LOCAL} instead'
        )
        require(not is_cycling(self.header(0), self.layout), refusal)
        print('Reading the full 16 MiB flash. This takes about two minutes.', flush=True)
        backup = self.read(0, self.layout.flash_size, 'flash.bin')
        self.verify(0, LOCAL / 'flash.bin')
        stock = backup[self.layout.slots[0] : self.layout.slots[0] + self.layout.slot_size]
        stock = stock[: image_length(stock)]
        require(not is_cycling(stock, self.layout), refusal)
        (LOCAL / 'stock.bin').write_bytes(stock)
        manifest = {
            'identity': identity,
            'flash_sha256': sha(backup),
            'stock_sha256': sha(stock),
            'stock_length': len(stock),
            'bootloader_region_sha256': sha(backup[: self.layout.metadata[0]]),
        }
        (LOCAL / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        self.reboot()
        print('Full backup verified; stock watchdog reset sent. Confirm the screen.')

    def baseline(self, identity, metadata):
        """Before any slot write: same device, intact backup, unchanged partition table and bootloader."""
        require((LOCAL / 'manifest.json').exists(), 'Run cycling backup before flashing')
        manifest = json.loads((LOCAL / 'manifest.json').read_text())
        backup = (LOCAL / 'flash.bin').read_bytes()
        stock = (LOCAL / 'stock.bin').read_bytes()
        require(identity == manifest['identity'], 'Connected device does not match the backup')
        require(
            sha(backup) == manifest['flash_sha256'] and sha(stock) == manifest['stock_sha256'],
            'Backup integrity check failed',
        )
        require(
            metadata[:SECTOR] == backup[self.layout.metadata[0] : self.layout.metadata[0] + SECTOR],
            'Partition table changed since backup',
        )
        boot = self.read(0, self.layout.metadata[0], 'bootloader-check.bin')
        require(sha(boot) == manifest['bootloader_region_sha256'], 'Bootloader changed since backup')

    def header(self, slot):
        header = self.read(self.layout.slots[slot], SECTOR, f'slot-{NAMES[slot].lower()}-header.bin')
        return header

    def contents(self, slot):
        """Describe what a slot holds from its first sector: cycling, stock or other."""
        return describe(self.header(slot), (LOCAL / 'stock.bin').read_bytes(), self.layout)

    def read_image(self, slot):
        """Read only as much of a slot as its image occupies, then require a valid cycling image."""
        size = 0x100000
        while True:
            data = self.read(self.layout.slots[slot], size, f'slot-{NAMES[slot].lower()}.bin')
            try:
                span = image_span(data)
                if span is not None and span <= len(data):
                    validate_cycling(data[:span], self.layout)
                    return data[:span]
                require(
                    size < self.layout.app_size and (span or 0) <= self.layout.app_size, 'Image exceeds the slot cap'
                )
            except RuntimeError as error:
                raise RuntimeError(f'Slot {NAMES[slot]} does not hold a valid cycling image: {error}') from None
            size = min(self.layout.app_size, max(size * 2, -(-(span or 0) // SECTOR) * SECTOR))

    def select(self, metadata, slot):
        valid = records(metadata, self.layout)
        latest, newest_offset = max(valid)
        require(latest < 0xFFFFFFFC, 'OTA sequence needs manual inspection before rollover')
        sequence = latest + 1
        if (sequence - 1) % 2 != slot:
            sequence += 1
        offset = self.layout.otadata + SECTOR if newest_offset == self.layout.otadata else self.layout.otadata
        sector = bytearray(b'\xff' * SECTOR)
        struct.pack_into('<I', sector, 0, sequence)
        struct.pack_into('<I', sector, 28, zlib.crc32(sector[:4], 0xFFFFFFFF))
        path = LOCAL / 'selection.bin'
        path.write_bytes(sector)
        expected = bytearray(metadata)
        expected[offset : offset + SECTOR] = sector
        self.write(self.layout.metadata[0] + offset, path)
        actual = self.read(*self.layout.metadata, 'after.bin')
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
        if info is not None and (reported == NAMES[slot] or (maintenance and reported is None)):
            detail = ' '.join(f'{key}={info[key]}' for key in ('commit', 'protocol') if key in info)
            source = 'INFO' if reported else 'OTA selection (the maintenance image does not report its slot)'
            print(f'Running slot {NAMES[slot]} per {source}; {detail}')
            if not maintenance:
                set_clock(resolve, LOCAL / f'{time.time_ns()}-clock.log')
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
            current, _ = self.connect()
            self.select(current, previous)
        except (RuntimeError, OSError, ValueError) as error:
            raise RuntimeError(
                f'Slot {NAMES[slot]} {problem}, and switching back to slot {NAMES[previous]} failed: {error}. '
                f'Reconnect, then run cycling rollback or cycling stock. Logs: {LOCAL}'
            ) from None
        if previous_probe is not None:
            back = console_info(resolve, seconds, LOCAL / f'{time.time_ns()}-console.log', previous_probe)
            state = f'slot {back.get("slot", NAMES[previous])} answers INFO' if back else 'its console is silent too'
        else:
            state = 'it holds stock; confirm the screen'
        raise RuntimeError(f'Slot {NAMES[slot]} {problem}. Reselected slot {NAMES[previous]}; {state}. Logs: {LOCAL}')

    def restore(self, backup):
        """Write the verified backup over every differing sector, then read the whole flash back."""
        print('Reading the full 16 MiB flash before restore. This takes about two minutes.', flush=True)
        current = self.read(0, self.layout.flash_size, 'restore-before.bin')
        plan = restore_plan(current, backup, self.layout)
        print(f'Restoring {sum(length for _, length in plan) // SECTOR} differing sectors in {len(plan)} writes.')
        run = LOCAL / 'restore-run.bin'
        for address, length in plan:
            run.write_bytes(backup[address : address + length])
            self.write(address, run)
        print('Reading the full flash back for sector-wise verification.', flush=True)
        after = self.read(0, self.layout.flash_size, 'restore-after.bin')
        bad = sectors_differing(after, backup)
        if bad:
            raise RuntimeError(f'{len(bad)} sectors differ from the backup, first at {bad[0]:#x}; rerun cycling stock')
        for name in ('restore-before.bin', 'restore-after.bin', 'restore-run.bin'):
            (LOCAL / name).unlink(missing_ok=True)
        self.reboot()
        print('Full stock restore verified; watchdog reset sent. Stock can remove USB: confirm the screen.')

    def stock_select(self, metadata):
        """Light path: boot slot A only while it still holds the verified stock image."""
        try:
            self.verify(self.layout.slots[0], LOCAL / 'stock.bin')
        except RuntimeError:
            raise RuntimeError('Slot A no longer holds stock; use cycling stock for the full restore') from None
        self.select(metadata, 0)
        print('Stock can remove USB: confirm the screen.')


def confirm_restore(confirmed, stdin=None):
    if confirmed:
        return
    stdin = sys.stdin if stdin is None else stdin
    require(stdin.isatty(), 'A full stock restore rewrites the whole 16 MiB flash; pass --confirm-restore')
    print('This rewrites the whole flash with the verified stock backup (eFuses untouched).')
    print('Cycling rides and settings on the device are lost. Type RESTORE to continue: ', end='', flush=True)
    require(stdin.readline().strip() == 'RESTORE', 'Restore cancelled')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('action', choices=['backup', 'flash', 'rollback', 'stock', 'stock-select', 'monitor'])
    add_port_argument(parser)
    parser.add_argument('--seconds', type=int, default=15, help='monitor duration')
    parser.add_argument(
        '--boot-seconds', type=int, default=CONSOLE_SECONDS, help='console deadline before reselecting the old slot'
    )
    parser.add_argument('--confirm-restore', action='store_true', help='stock: skip the interactive confirmation')
    args = parser.parse_args()
    port = selected_port(parser, args.port)

    def resolve():
        return select_port(args.port)

    if args.action == 'monitor':
        monitor(port, args.seconds)
        return
    layout = Layout.of(devices.selected())
    # Everything checkable offline is checked before interrupting the running firmware.
    candidate = ROOT / '.local/cycling.bin'
    if args.action == 'flash':
        image = candidate.read_bytes()
        validate_cycling(image, layout)
    if args.action == 'stock':
        confirm_restore(args.confirm_restore)
        manifest, backup = verified_backup(layout)
    device = Device(flasher_for(port, resolve, LOCAL), layout)
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
    running = selected_slot(metadata, layout)
    other = 1 - running
    running_header = device.header(running)
    running_contents = describe(running_header, (LOCAL / 'stock.bin').read_bytes(), layout)
    if args.action == 'rollback':
        image = device.read_image(other)
        print(f'Slot {NAMES[running]} is selected; returning to the cycling image in slot {NAMES[other]}.', flush=True)
    else:
        target_contents = device.contents(other)
        note = '; stock stays in the verified backup' if target_contents == 'stock' else ''
        print(
            f'Slot {NAMES[running]} ({running_contents}) is selected; writing slot {NAMES[other]} '
            f'(held {target_contents}{note}).',
            flush=True,
        )
        owned = device.flasher.digest(*layout.owned)
        device.write(layout.slots[other], candidate)
        device.verify(layout.slots[other], candidate)
        require(device.flasher.digest(*layout.owned) == owned, 'The flash changed the ride and settings reservations')
        print('Rides and settings are unchanged (MD5).', flush=True)
    device.boot(
        metadata,
        other,
        running,
        resolve,
        args.boot_seconds,
        previous_console=running_contents == 'cycling',
        # Each image's descriptor, not the environment, says which protocol its console speaks.
        maintenance=speaks_maintenance(image, layout),
        previous_maintenance=speaks_maintenance(running_header, layout),
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
    except (RuntimeError, OSError, devices.ManifestError) as error:
        sys.exit(str(error))
