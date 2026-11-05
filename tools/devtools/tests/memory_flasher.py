"""A Flasher over a bytearray: what the real Device runs against in tests."""

from pathlib import Path

from cycling_devtools.device.flasher import Chip, FlasherError


class MemoryFlasher:
    def __init__(self, memory, identity, secure_boot=False, flash_encryption=False):
        self.memory = bytearray(memory)
        self.identity, self.secure_boot, self.flash_encryption = identity, secure_boot, flash_encryption
        self.writes, self.enters, self.reboots = [], 0, 0
        self.boot_output: bytes | None = b''

    def _span(self, address, size):
        if address < 0 or address + size > len(self.memory):
            raise FlasherError('Region is outside the flash')
        return slice(address, address + size)

    def enter(self):
        self.enters += 1
        return Chip(self.identity, self.secure_boot, self.flash_encryption)

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
