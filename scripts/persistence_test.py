#!/usr/bin/env python3
"""Verify C606 preferences across temporary sessions and safe reflashes."""

import argparse
import json
import math
import subprocess
import time
from pathlib import Path

from debug import Device, ROOT, wake_if_dimmed


def flash():
    subprocess.run(['mise', 'run', 'flash'], cwd=ROOT, check=True)
    subprocess.run(['mise', 'run', 'monitor', '--', '--seconds', '3'], cwd=ROOT, check=True)


def inspect(port, directory):
    with Device(port, directory) as device:
        return device.command('STATE')['brightness']


def persist(port, directory, brightness):
    with Device(port, directory) as device:
        state = device.command(f'PERSIST {brightness}', timeout=15)
        if state['active'] or state['brightness'] != brightness:
            raise AssertionError(f'Persistence operation failed: {state}')


def temporary_change(port, directory, expected, changed):
    with Device(port, directory) as device:
        wake_if_dimmed(device)
        device.expect({'screen': 'home', 'brightness': expected})
        device.tap(80, 100)
        device.expect({'screen': 'settings'})
        x = math.ceil(24 + (changed - 5) * 192 / 95)
        device.drag([x, 170], [x, 170], seconds=.2, steps=2)
        device.expect({'brightness': changed})
        device.wait(1.3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', default='/dev/ttyACM0')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    output = args.output or ROOT / '.local/tests' / f'persistence-{time.time_ns()}'
    output.mkdir(parents=True, mode=0o700)

    original = inspect(args.port, output / 'initial')
    changed = 65 if original != 65 else 70
    temporary_change(args.port, output / 'temporary', original, changed)
    flash()
    after_temporary = inspect(args.port, output / 'after-temporary')
    if after_temporary != original:
        raise AssertionError('Temporary debug brightness persisted')

    persist(args.port, output / 'persist-new', changed)
    flash()
    restored = inspect(args.port, output / 'read-new')
    if restored != changed:
        raise AssertionError('Saved brightness was not restored')

    persist(args.port, output / 'restore-original', original)
    flash()
    final = inspect(args.port, output / 'read-original')
    if final != original:
        raise AssertionError('Original brightness was not restored')
    (output / 'summary.json').write_text(json.dumps({
        'original': original,
        'temporary': changed,
        'after_temporary_reflash': after_temporary,
        'persisted': changed,
        'after_persist_reflash': restored,
        'restored_original': final,
    }, indent=2) + '\n')
    print(f'Persistence evidence: {output}')


if __name__ == '__main__':
    main()
