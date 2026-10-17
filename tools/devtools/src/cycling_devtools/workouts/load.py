"""Load a structured workout from JSON onto VANA over USB.

The JSON names a workout and its steps; nested `repeat` groups become the
device's repeat blocks:

    {
      "name": "OVER UNDERS",
      "steps": [
        {"intensity": "warmup", "minutes": 10, "ftp": [50, 65]},
        {"repeat": 3, "steps": [
          {"intensity": "work", "minutes": 2, "ftp": [92, 96], "note": "UNDER"},
          {"intensity": "work", "seconds": 60, "ftp": [104, 108]}
        ]},
        {"intensity": "cooldown", "km": 2, "heart_zone": 1}
      ]
    }

A step's length is one of `seconds`, `minutes`, `metres` or `km` (100 m at
least), or
`"open": true` (until the rider presses MARK). Its target is at most one of
`ftp` [low, high] %, `watts` [low, high], `heart_zone` 1-5, `lthr` [low,
high] %, `cadence` [low, high] rpm or `ramp` {start, step, every}; none is a
free ride. `test` may be `twenty` or `ramp`. The encoding is codec version 1
(`apps/vana/src/workout/codec.rs`).

The workout goes onto the MMC's FILES volume as `workouts/<name>.vw` through
the volume transfer session, which checks the file's CRC on commit and reads
it back. VANA lists `workouts/` when its picker opens; this tool also asks it
to list again (`WORKOUT LIST`). Run `mise run files -- format --confirm` once
on a device whose FILES volume is not formatted yet.
"""

import argparse
import json
import re
import struct
import time
import zlib
from pathlib import Path

from cycling_devtools.device import volumes
from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT

MAGIC = b'VW'
VERSION = 1
NAME_BYTES = 20
MAX_STEPS = 12
MAX_BLOCKS = 8
FOLDER = 'workouts/'
SUFFIX = '.vw'
# FILES names: 1 to 40 bytes of A-Z a-z 0-9 . _ - /.
NAME_MAX = 40
MIN_DISTANCE_M = 100

INTENSITIES = ('warmup', 'work', 'recovery', 'cooldown', 'test')
TESTS = {None: 0, 'twenty': 1, 'ramp': 2}
LENGTHS = {'seconds': (0, 1), 'minutes': (0, 60), 'metres': (1, 1), 'km': (1, 1000)}
RANGES = {'ftp': (1, 0xFF), 'watts': (2, 0xFFFF), 'lthr': (4, 0xFF), 'cadence': (5, 0xFF)}


def text(value, what):
    """Upper-case printable ASCII of at most NAME_BYTES, as the device keeps names."""
    if not isinstance(value, str):
        raise ValueError(f'{what} must be text')
    encoded = value.upper().encode('ascii', errors='replace')
    if len(encoded) > NAME_BYTES or any(not 0x20 <= byte <= 0x7E for byte in encoded):
        raise ValueError(f'{what} must be at most {NAME_BYTES} printable ASCII characters: {value!r}')
    return encoded


def whole(value, what, low, high):
    if type(value) is not int or not low <= value <= high:
        raise ValueError(f'{what} must be a whole number in {low}..{high}')
    return value


def length(step):
    kinds = [key for key in (*LENGTHS, 'open') if key in step]
    if len(kinds) != 1:
        raise ValueError(f'a step needs exactly one length of {", ".join((*LENGTHS, "open"))}: {step}')
    if kinds[0] == 'open':
        if step['open'] is not True:
            raise ValueError('"open" must be true')
        return 2, 0
    kind, scale = LENGTHS[kinds[0]]
    amount = step[kinds[0]]
    if isinstance(amount, float) and amount * scale == int(amount * scale):
        amount = int(amount * scale)
    elif type(amount) is int:
        amount *= scale
    else:
        raise ValueError(f'{kinds[0]} must be whole {"seconds" if kind == 0 else "metres"}')
    if kind == 0:
        return kind, whole(amount, kinds[0], 1, 0xFFFF)
    # The device refuses distance steps under 100 m (`MIN_DISTANCE_M`).
    return kind, whole(amount, f'{kinds[0]} in metres', MIN_DISTANCE_M, 0xFFFFFFFF)


def target(step):
    kinds = [key for key in (*RANGES, 'heart_zone', 'ramp') if key in step]
    if len(kinds) > 1:
        raise ValueError(f'a step has at most one target: {step}')
    if not kinds:
        return 0, (0, 0, 0)
    kind = kinds[0]
    value = step[kind]
    if kind == 'heart_zone':
        return 3, (whole(value, 'heart_zone', 1, 5), 0, 0)
    if kind == 'ramp':
        if not isinstance(value, dict) or set(value) != {'start', 'step', 'every'}:
            raise ValueError('ramp needs start, step and every')
        return 6, tuple(whole(value[key], f'ramp {key}', 1, 0xFFFF) for key in ('start', 'step', 'every'))
    code, top = RANGES[kind]
    if not isinstance(value, list) or len(value) != 2:
        raise ValueError(f'{kind} needs [low, high]')
    low, high = (whole(bound, kind, 1, top) for bound in value)
    if low > high:
        raise ValueError(f'{kind} low is above high')
    return code, (low, high, 0)


def encode_step(step):
    if step.get('intensity') not in INTENSITIES:
        raise ValueError(f'intensity must be one of {", ".join(INTENSITIES)}: {step}')
    unknown = set(step) - {'intensity', 'note', 'open', *LENGTHS, *RANGES, 'heart_zone', 'ramp'}
    if unknown:
        raise ValueError(f'unknown step fields {sorted(unknown)}')
    note = text(step.get('note', ''), 'note')
    length_kind, amount = length(step)
    target_kind, values = target(step)
    fixed = struct.pack(
        '<BBIB3HBB', INTENSITIES.index(step['intensity']), length_kind, amount, target_kind, *values, 0, len(note)
    )
    return fixed + note


def flatten(items):
    """Steps in order and repeat blocks as (first, steps, runs)."""
    steps, blocks = [], []
    for item in items:
        if not isinstance(item, dict):
            raise ValueError('steps must be objects')
        if 'repeat' in item:
            if set(item) != {'repeat', 'steps'}:
                raise ValueError('a repeat group has only repeat and steps')
            inner = item['steps']
            if not isinstance(inner, list) or not inner or any('repeat' in step for step in inner):
                raise ValueError('a repeat group holds one or more plain steps')
            blocks.append((len(steps), len(inner), whole(item['repeat'], 'repeat', 1, 0xFF)))
            steps.extend(inner)
        else:
            steps.append(item)
    return steps, blocks


def encode(workout):
    """A workout from JSON as the device's upload bytes, CRC32 trailer included."""
    if not isinstance(workout, dict) or not isinstance(workout.get('steps'), list):
        raise ValueError('a workout is an object with a name and steps')
    unknown = set(workout) - {'name', 'test', 'steps'}
    if unknown:
        raise ValueError(f'unknown workout fields {sorted(unknown)}')
    if workout.get('test') not in TESTS:
        raise ValueError('test must be twenty or ramp')
    name = text(workout.get('name', ''), 'name')
    if not name.strip():
        raise ValueError('a workout needs a name')
    steps, blocks = flatten(workout['steps'])
    if not 1 <= len(steps) <= MAX_STEPS:
        raise ValueError(f'a workout has 1..{MAX_STEPS} steps')
    if len(blocks) > MAX_BLOCKS:
        raise ValueError(f'a workout has at most {MAX_BLOCKS} repeat groups')
    body = bytearray(MAGIC)
    body += bytes([VERSION, TESTS[workout.get('test')], len(name)]) + name
    body += bytes([len(steps), len(blocks)])
    for step in steps:
        body += encode_step(step)
    for block in blocks:
        body += bytes(block)
    return bytes(body) + struct.pack('<I', zlib.crc32(body))


def file_name(stem):
    """The FILES name of a workout called `stem`: `workouts/<stem>.vw`."""
    safe = re.sub(r'[^A-Za-z0-9._-]+', '-', stem).strip('-.')
    safe = safe[: NAME_MAX - len(FOLDER) - len(SUFFIX)]
    if not safe:
        raise ValueError(f'no usable file name in {stem!r}')
    return f'{FOLDER}{safe}{SUFFIX}'


def upload(connection, name, data, scratch):
    """Put `data` on FILES as `name`, then have VANA list its workouts again."""
    path = scratch / name.replace('/', '__')
    path.write_bytes(data)
    transfer = volumes.open_session(connection)
    try:
        entry = volumes.files_put(transfer, name, path)
    finally:
        transfer.call('end')
    reply = connection.terminal_command('WORKOUT LIST')
    if reply['status'] != 'OK':
        raise RuntimeError(f'WORKOUT LIST: {reply["status"]} {reply["data"]}'.rstrip())
    if f'{name}:' not in reply['data']:
        raise RuntimeError(f'VANA did not list {name}: {reply["data"]}')
    return {**entry, 'list': reply['data']}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('workout', type=Path, help='workout JSON file')
    parser.add_argument('--name', help='file name stem (default: the JSON file name)')
    parser.add_argument('--dry-run', action='store_true', help='print the file it would put, without a device')
    add_port_argument(parser)
    args = parser.parse_args()
    data = encode(json.loads(args.workout.read_text()))
    name = file_name(args.name or args.workout.stem)
    if args.dry_run:
        print(json.dumps({'name': name, 'length': len(data), 'crc32': f'{zlib.crc32(data):08x}'}))
        return
    port = selected_port(parser, args.port)
    scratch = ROOT / '.local/tests'
    scratch.mkdir(parents=True, exist_ok=True)
    with UsbConnection(port, scratch / f'workout-load-{time.time_ns()}.log') as connection:
        print(json.dumps(upload(connection, name, data, scratch)))


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        raise SystemExit(str(error)) from None
