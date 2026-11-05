"""How Device reaches the chip's ROM: one protocol, one backend per way to the tty.

firmware.Device keeps every policy (what to read, what to write, what must
match, when to reboot, how to recover, what a security verdict allows). A
Flasher only moves bytes and reports chip facts, and owns the integrity of
what it moves, as esptool does locally.
"""

import io
import os
import re
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Protocol


@dataclass(frozen=True)
class Flash:
    manufacturer: int
    device: int
    size: int


@dataclass(frozen=True)
class Usb:
    vendor_id: int
    product_id: int


@dataclass(frozen=True)
class Security:
    secure_boot: bool
    flash_encryption: bool


@dataclass(frozen=True)
class Chip:
    """What the ROM says about the chip at entry, in esptool's words."""

    identity: str  # the manifest's form: 'aa:bb:cc:dd:ee:ff'
    family: str  # esptool's CHIP_NAME, such as 'ESP32-S3'
    model: str  # the chip description less its revision, such as 'ESP32-S3 (QFN56)'
    revision: str  # as the description words it: 'v0.2', or '1' from esptool-js on an ESP32
    features: tuple[str, ...]
    crystal_mhz: int
    flash: Flash
    usb: Usb | None  # None when the port reports no USB ids
    security: Security | None  # None when the ROM has no security-info command (ESP8266, ESP32)

    @property
    def accepts_unsigned(self) -> bool | None:
        """Secure boot and flash encryption are both off; None when the ROM cannot say."""
        if self.security is None:
            return None
        return not self.security.secure_boot and not self.security.flash_encryption


class Flasher(Protocol):
    def enter(self) -> Chip:
        """Download mode, ready for the other verbs. Idempotent while held."""

    def read(self, address: int, size: int, path: Path) -> None:
        """path holds exactly size bytes the chip's own digest vouches for."""

    def write(self, address: int, path: Path) -> None:
        """Written, and the chip's digest of the region matches the file."""

    def verify(self, address: int, path: Path) -> bool:
        """Whether the chip's MD5 of len(file) bytes at address equals the file's."""

    def reboot(self, seconds: float) -> bytes | None:
        """Watchdog reset into the selected app; then up to seconds of boot output, or None if USB never came back."""


class FlasherError(RuntimeError):
    """Messages carry no device identity."""


# The ROMs without GET_SECURITY_INFO; esptool's get-security-info fails on them.
NO_SECURITY_INFO = frozenset({'ESP8266', 'ESP32'})
REVISION = re.compile(r'^(.*\S)\s*\(revision ([^)]+)\)$')
SIZE_UNITS = {'KB': 1024, 'MB': 1024 * 1024}


def line(pattern, output):
    found = re.search(pattern, output, re.M)
    if found is None:
        raise FlasherError('esptool did not describe the chip')
    return found


def chip_from(described, security, usb=None):
    """A Chip from esptool's connect banner and flash-id, then get-security-info's output or None."""
    # Chips with an EUI-64 print it as MAC and the six-byte base address as BASE MAC.
    mac = re.search(r'^BASE MAC:\s*([0-9a-f:]{17})\s*$', described, re.I | re.M) or re.search(
        r'^MAC:\s*([0-9a-f:]{17})\s*$', described, re.I | re.M
    )
    if mac is None:
        raise FlasherError('Could not establish device identity')
    description = line(r'^Chip type:\s+(.+?)\s*$', described)[1]
    revised = REVISION.match(description)
    size = line(r'^Detected flash size:\s*(\d+)(KB|MB)\s*$', described)
    return Chip(
        identity=mac[1].lower(),
        family=line(r'^Connected to (\S+) on ', described)[1],
        model=revised[1] if revised else description,
        revision=revised[2] if revised else 'unknown',
        features=tuple(line(r'^Features:\s+(.+?)\s*$', described)[1].split(', ')),
        crystal_mhz=int(line(r'^Crystal frequency:\s+(\d+)MHz', described)[1]),
        flash=Flash(
            manufacturer=int(line(r'^Manufacturer:\s*([0-9a-f]+)\s*$', described)[1], 16),
            device=int(line(r'^Device:\s*([0-9a-f]+)\s*$', described)[1], 16),
            size=int(size[1]) * SIZE_UNITS[size[2]],
        ),
        usb=usb,
        # Anything but the explicit Disabled line counts as enabled, so unreadable output is refused.
        security=None
        if security is None
        else Security(
            secure_boot='Secure Boot: Disabled' not in security,
            flash_encryption='Flash Encryption: Disabled' not in security,
        ),
    )


def usb_of(port):
    """The tty's USB ids from sysfs, as pyserial lists them; None for a port it does not know."""
    from serial.tools import list_ports

    device = os.path.realpath(port)
    for info in list_ports.comports():
        if os.path.realpath(info.device) == device and info.vid is not None and info.pid is not None:
            return Usb(vendor_id=info.vid, product_id=info.pid)
    return None


class EsptoolFlasher:
    """One esptool process per verb, on a local tty.

    The firmware tasks name the C606's esp32s3; `cycling flasher` passes
    'auto', which lets esptool detect the family and pick its reset.
    """

    def __init__(
        self, port: str, resolve: Callable[[], str], log_dir: Path, chip: Literal['esp32s3', 'auto'] = 'esp32s3'
    ):
        self.port, self.resolve, self.log_dir, self.chip = port, resolve, log_dir, chip
        self.count = 0

    def _run(self, *args, before='no-reset', after='no-reset'):
        self.count += 1
        command = [
            sys.executable,
            '-m',
            'esptool',
            '--chip',
            self.chip,
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
        (self.log_dir / f'{time.time_ns()}-{self.count}.log').write_text(output)
        if result.returncode != 0:
            raise FlasherError(f'esptool failed; inspect private logs in {self.log_dir}')
        return output

    def enter(self):
        # A watchdog reset re-enumerates the tty, so ask again once any earlier run has used it.
        if self.count:
            self.port = self.resolve()
        # esptool's default-reset is its USB-Serial-JTAG reset on 303a:1001, its classic one on a bridge.
        described = self._run(
            '--no-stub', 'flash-id', before='usb-reset' if self.chip == 'esp32s3' else 'default-reset'
        )
        family = re.search(r'^Connected to (\S+) on ', described, re.M)
        security = (
            None
            if family is not None and family[1] in NO_SECURITY_INFO
            else self._run('--no-stub', 'get-security-info')
        )
        return chip_from(described, security, usb_of(self.port))

    def read(self, address, size, path):
        self._run('read-flash', hex(address), hex(size), path)

    def write(self, address, path):
        self._run(
            'write-flash', '--flash-mode', 'keep', '--flash-freq', 'keep', '--flash-size', 'keep', hex(address), path
        )

    def verify(self, address, path):
        return 'Verification successful (digest matched).' in self._run('verify-flash', hex(address), path)

    def reboot(self, seconds):
        # RTS hard-reset left this C606 in ROM download mode. The watchdog
        # reset boots the selected app, and stock can then remove the USB port.
        # read-mac is a command every family answers; on one without a watchdog reset esptool pulses RTS.
        self._run('read-mac', after='watchdog-reset')
        captured = io.BytesIO()
        try:
            monitor(self.port, seconds, output=captured)
        except OSError:
            # Stock removes the port; whatever arrived before it went still gets judged.
            return captured.getvalue() or None
        return captured.getvalue()


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
