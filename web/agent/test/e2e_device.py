"""Device(firmware.flasher_for(...)) through the visitor's page, as a firmware task runs it.

Run by e2e-page.ts under `cycling remote CODE --`. Private state goes to a
scratch directory, never to the checkout's .local/device. Prints one JSON
line per step; the exit code says whether every step held.

    python e2e_device.py flow <scratch>      cycling flasher info|read|verify, then Device: enter, read 0x8000,
                                             write+verify a scratch region, reboot
    python e2e_device.py cli <scratch>       cycling flasher info|read|verify only, then Device's refusal of a chip
                                             whose ROM cannot report its security
    python e2e_device.py long-read <scratch> a 16 MiB read, for the chaos run to kill midway
    python e2e_device.py bench <scratch>     one timed read of E2E_BENCH_MIB MiB
"""

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from cycling_devtools.device import firmware
from cycling_devtools.workspace import ROOT

SCRATCH_REGION = 0xF00000
SCRATCH_BYTES = 0x40000


def step(name, **facts):
    print(json.dumps({'step': name, **facts}), flush=True)


def flasher_cli(scratch, *argv):
    """One `cycling flasher` verb in its own process, so its lease ends when it exits; logs stay in scratch."""
    script = (
        'import sys; from pathlib import Path; from cycling_devtools.device import flasher_cli as cli; '
        'cli.LOG_DIR = Path(sys.argv[1]); sys.exit(cli.main(sys.argv[2:]))'
    )
    result = subprocess.run([sys.executable, '-c', script, scratch, *argv], check=False, capture_output=True, text=True)
    firmware.require(result.returncode == 0, f'cycling flasher {argv[0]} failed: {result.stderr.strip()}')
    return result.stdout


def cli_flow(scratch):
    described = json.loads(flasher_cli(scratch, 'info'))
    firmware.require('02:00:00' not in json.dumps(described), 'info printed the MAC')
    step('flasher info', **described)
    (ROOT / '.local').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=ROOT / '.local') as local:
        region = Path(local) / 'table.bin'
        flasher_cli(scratch, 'read', '0x8000', '0x1000', str(region))
        step('flasher read', bytes=len(region.read_bytes()))
        verdict = flasher_cli(scratch, 'verify', '0x8000', str(region))
        step('flasher verify', matched='matches' in verdict)


def main(mode, scratch):
    firmware.LOCAL = Path(scratch)
    port = os.environ['CYCLING_PORT']
    if mode in ('flow', 'cli'):
        cli_flow(scratch)
    if mode == 'cli':
        try:
            firmware.Device(firmware.flasher_for(port, lambda: port, firmware.LOCAL)).connect()
        except RuntimeError as error:
            step('device refuses', reason=str(error))
            return
        raise SystemExit('Device accepted a chip whose security is unknown')
    device = firmware.Device(firmware.flasher_for(port, lambda: port, firmware.LOCAL))
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
