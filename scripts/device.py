# /// script
# requires-python = ">=3.12"
# dependencies = ["esptool==5.4.0"]
# ///
"""C606 backup, application-only flashing, and stock boot selection."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import time
import zlib

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / ".local/device"
FLASH_SIZE = 0x1000000
SLOT_SIZE = 0x73A000
SETTINGS_SIZE = 0x2000
RIDE_SIZE = 0x100000
APP_SIZE = SLOT_SIZE - SETTINGS_SIZE - RIDE_SIZE
SLOTS = (0x20000, 0x760000)


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def image_length(data):
    """Validate a stock-compatible ESP32-S3 application and return its extent."""
    require(len(data) >= 24 and data[0] == 0xE9, "Invalid ESP image header")
    require(1 <= data[1] <= 16 and struct.unpack_from("<H", data, 12)[0] == 9,
            "Image must target ESP32-S3")
    offset, checksum = 24, 0xEF
    for _ in range(data[1]):
        require(offset + 8 <= len(data), "Truncated segment header")
        size = struct.unpack_from("<I", data, offset + 4)[0]
        offset += 8
        require(offset + size <= len(data), "Truncated segment")
        for value in data[offset:offset + size]:
            checksum ^= value
        offset += size
    end = (offset + 16) & ~15
    require(end <= len(data) and data[end - 1] == checksum, "ESP checksum mismatch")
    require(data[23] == 1, "Expected appended SHA-256")
    require(data[end:end + 32] == hashlib.sha256(data[:end]).digest(), "ESP SHA-256 mismatch")
    return end + 32


def validate_candidate(data):
    require(image_length(data) == len(data), "Image has bytes outside its validated image extent")
    require(len(data) <= APP_SIZE, "Image overlaps the reserved slot-B ride/settings storage")


def partitions(metadata):
    expected = [("nvs", 1, 2, 0x9000, 0x4000), ("otadata", 1, 0, 0xD000, 0x2000),
                ("phy_init", 1, 1, 0xF000, 0x1000), ("coredump", 1, 3, 0x10000, 0x10000),
                ("ota_0", 0, 0x10, SLOTS[0], SLOT_SIZE), ("ota_1", 0, 0x11, SLOTS[1], SLOT_SIZE)]
    entries = []
    for offset in range(0, 0xC00, 32):
        entry = metadata[offset:offset + 32]
        magic = struct.unpack_from("<H", entry)[0]
        if magic == 0xEBEB:
            require(hashlib.md5(metadata[:offset]).digest() == entry[16:], "Partition MD5 mismatch")
            break
        require(magic == 0x50AA, "Unexpected partition table")
        kind, subtype, address, size, label, flags = struct.unpack("<BBII16sI", entry[2:])
        require(flags == 0, "Encrypted or flagged partition is unsupported")
        entries.append((label.rstrip(b"\0").decode(), kind, subtype, address, size))
    # Stock labels can differ; layout and types must match exactly.
    require([e[1:] for e in entries] == [e[1:] for e in expected], "Unsupported C606 partition layout")


def records(metadata):
    result = []
    for offset in (0x5000, 0x6000):
        record = metadata[offset:offset + 32]
        seq, state, crc = struct.unpack_from("<I20xII", record)
        valid = seq not in (0, 0xFFFFFFFF) and state not in (3, 4)
        valid &= zlib.crc32(record[:4], 0xFFFFFFFF) == crc
        if valid:
            result.append((seq, offset))
    require(result, "No valid OTA selection record")
    return result


class Device:
    def __init__(self, port):
        self.port = port
        self.count = 0
        LOCAL.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(LOCAL, 0o700)

    def run(self, *args, before="no-reset", after="no-reset"):
        self.count += 1
        command = [sys.executable, "-m", "esptool", "--chip", "esp32s3", "--port", self.port,
                   "--before", before, "--after", after, *map(str, args)]
        result = subprocess.run(command, capture_output=True, text=True)
        output = result.stdout + result.stderr
        (LOCAL / f"{time.time_ns()}-{self.count}.log").write_text(output)
        require(result.returncode == 0, f"esptool failed; inspect private logs in {LOCAL}")
        return output

    def read(self, address, size, name):
        path = LOCAL / name
        output = self.run("read-flash", hex(address), hex(size), path)
        data = path.read_bytes()
        require(len(data) == size, "Short flash read")
        return data, output

    def connect(self):
        output = self.run("--no-stub", "get-security-info", before="usb-reset")
        require("Secure Boot: Disabled" in output and "Flash Encryption: Disabled" in output,
                "This procedure requires secure boot and flash encryption disabled")
        metadata, output = self.read(0x8000, 0x8000, "before.bin")
        mac = re.search(r"MAC:\s*([0-9a-f:]{17})", output, re.I)
        require(mac is not None, "Could not establish device identity")
        partitions(metadata)
        records(metadata)
        return metadata, mac[1].lower()

    def verify(self, address, path):
        output = self.run("verify-flash", hex(address), path)
        require("Verification successful (digest matched)." in output, "Device digest mismatch")

    def write(self, address, path):
        self.run("write-flash", "--flash-mode", "keep", "--flash-freq", "keep", "--flash-size", "keep",
                 hex(address), path)

    def reboot(self):
        self.run("get-security-info", after="hard-reset")
        # On this C606, stock only became visible after opening USB with both
        # control lines released. Keep the port open briefly and save boot output.
        path = LOCAL / f"{time.time_ns()}-boot.log"
        with path.open("wb") as output:
            monitor(self.port, 3, output=output)

    def backup(self, metadata, identity):
        require((max(records(metadata))[0] - 1) % 2 == 0, "Boot stock slot A before making the baseline backup")
        require(not (LOCAL / "manifest.json").exists(), "A backup already exists; it will not be overwritten")
        print("Reading the full 16 MiB flash. This takes about two minutes.", flush=True)
        backup, _ = self.read(0, FLASH_SIZE, "flash.bin")
        self.verify(0, LOCAL / "flash.bin")
        stock = backup[SLOTS[0]:SLOTS[0] + SLOT_SIZE]
        stock = stock[:image_length(stock)]
        (LOCAL / "stock.bin").write_bytes(stock)
        manifest = {"identity": identity, "flash_sha256": sha(backup), "stock_sha256": sha(stock),
                    "stock_length": len(stock), "bootloader_region_sha256": sha(backup[:0x8000])}
        (LOCAL / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        self.reboot()
        print("Full backup verified; stock reset and USB capture complete. Confirm the screen.")

    def baseline(self, identity, metadata):
        require((LOCAL / "manifest.json").exists(), "Run mise run backup before flashing")
        manifest = json.loads((LOCAL / "manifest.json").read_text())
        backup = (LOCAL / "flash.bin").read_bytes()
        stock = (LOCAL / "stock.bin").read_bytes()
        require(identity == manifest["identity"], "Connected device does not match the backup")
        require(sha(backup) == manifest["flash_sha256"] and sha(stock) == manifest["stock_sha256"],
                "Backup integrity check failed")
        require(metadata[:0x1000] == backup[0x8000:0x9000], "Partition table changed since backup")
        boot, _ = self.read(0, 0x8000, "bootloader-check.bin")
        require(sha(boot) == manifest["bootloader_region_sha256"], "Bootloader changed since backup")
        self.verify(SLOTS[0], LOCAL / "stock.bin")

    def select(self, metadata, slot):
        valid = records(metadata)
        latest, newest_offset = max(valid)
        require(latest < 0xFFFFFFFC, "OTA sequence needs manual inspection before rollover")
        sequence = latest + 1
        if (sequence - 1) % 2 != slot:
            sequence += 1
        offset = 0x6000 if newest_offset == 0x5000 else 0x5000
        sector = bytearray(b"\xff" * 0x1000)
        struct.pack_into("<I", sector, 0, sequence)
        struct.pack_into("<I", sector, 28, zlib.crc32(sector[:4], 0xFFFFFFFF))
        path = LOCAL / "selection.bin"
        path.write_bytes(sector)
        expected = bytearray(metadata)
        expected[offset:offset + 0x1000] = sector
        self.write(0x8000 + offset, path)
        actual, _ = self.read(0x8000, 0x8000, "after.bin")
        require(actual == expected, "Boot selection readback does not match the planned change")
        self.reboot()
        print(f"Slot {'A (stock)' if slot == 0 else 'B (cycling)'} selected; reset and USB capture complete. Confirm the screen.")


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["backup", "flash", "stock", "monitor"])
    parser.add_argument("--port", default=os.environ.get("CYCLING_PORT", "/dev/ttyACM0"))
    parser.add_argument("--seconds", type=int, default=15)
    args = parser.parse_args()
    if args.action == "monitor":
        monitor(args.port, args.seconds)
        return
    device = Device(args.port)
    # Validate the candidate before interrupting stock firmware.
    candidate = ROOT / ".local/cycling.bin"
    if args.action == "flash":
        image = candidate.read_bytes()
        validate_candidate(image)
    metadata, identity = device.connect()
    if args.action == "backup":
        device.backup(metadata, identity)
        return
    device.baseline(identity, metadata)
    if args.action == "flash":
        print("Writing slot B; stock slot A is preserved.", flush=True)
        device.write(SLOTS[1], candidate)
        device.verify(SLOTS[1], candidate)
        device.verify(SLOTS[0], LOCAL / "stock.bin")
    device.select(metadata, 1 if args.action == "flash" else 0)


if __name__ == "__main__":
    try:
        LOCAL.parent.mkdir(parents=True, exist_ok=True)
        with (LOCAL.parent / "usb.lock").open("w") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise RuntimeError("Another cycling device task is using USB; wait until it finishes") from None
            main()
    except (RuntimeError, OSError) as error:
        sys.exit(str(error))
