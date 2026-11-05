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
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol


@dataclass(frozen=True)
class Chip:
    identity: str  # the manifest's form: 'aa:bb:cc:dd:ee:ff'
    secure_boot: bool
    flash_encryption: bool


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


def flasher_for(
    port: str, resolve: Callable[[], str], log_dir: Path, environ: Mapping[str, str] = os.environ
) -> Flasher:
    """The one backend choice."""
    return EsptoolFlasher(port, resolve, log_dir, environ)


def esptool_port(port, environ):
    # A pty has no DTR/RTS lines, so esptool reaches a bridged device through its rfc2217 URL.
    bridged, url = environ.get('CYCLING_PORT'), environ.get('CYCLING_ESPTOOL_PORT')
    return url if bridged and url and port == bridged else port


def chip_from(output):
    """Chip facts from esptool's get-security-info text."""
    mac = re.search(r'MAC:\s*([0-9a-f:]{17})', output, re.I)
    if mac is None:
        raise FlasherError('Could not establish device identity')
    # Anything but the explicit Disabled line counts as enabled, so unreadable output is refused.
    return Chip(
        identity=mac[1].lower(),
        secure_boot='Secure Boot: Disabled' not in output,
        flash_encryption='Flash Encryption: Disabled' not in output,
    )


class EsptoolFlasher:
    """One esptool process per verb, on a local tty."""

    def __init__(self, port: str, resolve: Callable[[], str], log_dir: Path, environ: Mapping[str, str] = os.environ):
        self.port, self.resolve, self.log_dir, self.environ = port, resolve, log_dir, environ
        self.count = 0

    def _run(self, *args, before='no-reset', after='no-reset'):
        self.count += 1
        command = [
            sys.executable,
            '-m',
            'esptool',
            '--chip',
            'esp32s3',
            '--port',
            esptool_port(self.port, self.environ),
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
        return chip_from(self._run('--no-stub', 'get-security-info', before='usb-reset'))

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
        self._run('get-security-info', after='watchdog-reset')
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
