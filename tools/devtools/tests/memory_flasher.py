"""A Flasher over a bytearray: what the real Device runs against in tests."""

from pathlib import Path

from cycling_devtools.device.flasher import Chip, Flash, FlasherError, Security, Usb

UNSIGNED = Security(secure_boot=False, flash_encryption=False)


def c606(identity, security=UNSIGNED):
    """The C606's ESP32-S3 as esptool describes it."""
    return Chip(
        identity=identity,
        family='ESP32-S3',
        model='ESP32-S3 (QFN56)',
        revision='v0.2',
        features=('Wi-Fi', 'BT 5 (LE)', 'Dual Core + LP Core', '240MHz'),
        crystal_mhz=40,
        flash=Flash(manufacturer=0x20, device=0x4018, size=0x1000000),
        usb=Usb(vendor_id=0x303A, product_id=0x1001),
        security=security,
    )


class MemoryFlasher:
    def __init__(self, memory, identity):
        self.memory = bytearray(memory)
        self.identity = identity
        # None: a ROM without security info, as on the ESP32.
        self.security: Security | None = UNSIGNED
        self.writes, self.enters, self.reboots = [], 0, 0
        self.boot_output: bytes | None = b''

    def _span(self, address, size):
        if address < 0 or address + size > len(self.memory):
            raise FlasherError('Region is outside the flash')
        return slice(address, address + size)

    def enter(self):
        self.enters += 1
        return c606(self.identity, self.security)

    def read(self, address, size, path: Path):
        path.write_bytes(self.memory[self._span(address, size)])

    def write(self, address, path: Path):
        data = path.read_bytes()
        self.memory[self._span(address, len(data))] = data
        self.writes.append((address, len(data)))

    def verify(self, address, path: Path):
        data = path.read_bytes()
        return self.memory[self._span(address, len(data))] == data

    def reboot(self, seconds):
        self.reboots += 1
        return self.boot_output
