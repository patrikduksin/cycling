"""Device(flasher_for(...)) through the visitor's page, as a firmware task runs it.

Run by e2e-page.ts under `cycling remote CODE --`. Private state goes to a
scratch directory, never to the checkout's .local/device. Prints one JSON
line per step; the exit code says whether every step held.

    python e2e_device.py flow <scratch>      enter, read 0x8000, write+verify a scratch region, reboot
    python e2e_device.py long-read <scratch> a 16 MiB read, for the chaos run to kill midway
    python e2e_device.py bench <scratch>     one timed read of E2E_BENCH_MIB MiB
"""

import hashlib
import json
import os
import sys
import time
from pathlib import Path

from cycling_devtools.device import firmware
from cycling_devtools.device.flasher import flasher_for

SCRATCH_REGION = 0xF00000
SCRATCH_BYTES = 0x40000


def step(name, **facts):
    print(json.dumps({'step': name, **facts}), flush=True)


def main(mode, scratch):
    firmware.LOCAL = Path(scratch)
    port = os.environ['CYCLING_PORT']
    device = firmware.Device(flasher_for(port, lambda: port, firmware.LOCAL))
    started = time.monotonic()
    metadata, identity = device.connect(strict=True)
    step('enter', seconds=round(time.monotonic() - started, 2), identity_length=len(identity))
    step('read 0x8000', bytes=len(metadata), sha256=hashlib.sha256(metadata).hexdigest())
    if mode == 'bench':
        size = int(os.environ['E2E_BENCH_MIB']) * 1024 * 1024
        started = time.monotonic()
        device.read(0, size, 'bench.bin')
        seconds = time.monotonic() - started
        step('bench read', bytes=size, seconds=round(seconds, 1), kb_per_s=round(size / seconds / 1000))
        return
    if mode == 'long-read':
        (Path(scratch) / 'pid').write_text(str(os.getpid()))
        device.read(0, firmware.FLASH_SIZE, 'flash.bin')
        step('long read finished')
        return
    image = Path(scratch) / 'scratch.bin'
    image.write_bytes(os.urandom(SCRATCH_BYTES))
    started = time.monotonic()
    device.write(SCRATCH_REGION, image)
    device.verify(SCRATCH_REGION, image)
    step('write+verify', address=hex(SCRATCH_REGION), bytes=SCRATCH_BYTES, seconds=round(time.monotonic() - started, 2))
    readback = device.read(SCRATCH_REGION, SCRATCH_BYTES, 'readback.bin')
    firmware.require(readback == image.read_bytes(), 'Readback differs from the written image')
    step('readback', matched=True)
    device.reboot()
    logs = sorted(Path(scratch).glob('*-boot.log'))
    firmware.require(logs, 'No boot capture')
    step('reboot', captured_bytes=len(logs[-1].read_bytes()))


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
