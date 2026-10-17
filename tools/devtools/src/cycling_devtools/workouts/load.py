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
(`apps/vana/src/workout/codec.rs`). Uploads live in RAM on this firmware: a
power-off forgets them, the built-ins stay.
"""

import argparse
import json
import struct
import time
import zlib
from pathlib import Path

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT

MAGIC = b'VW'
VERSION = 1
NAME_BYTES = 20
MAX_STEPS = 12
MAX_BLOCKS = 8
UPLOAD_SLOTS = 4
# The device's DATA chunk: 48 bytes as 96 hex digits fits a terminal line.
CHUNK = 48
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


def commands(slot, data):
    """The terminal commands that upload `data` into `slot`."""
    if not 0 <= slot < UPLOAD_SLOTS:
        raise ValueError(f'slot must be 0..{UPLOAD_SLOTS - 1}')
    yield f'WORKOUT BEGIN {slot} {len(data)} {zlib.crc32(data):08x}'
    for offset in range(0, len(data), CHUNK):
        yield f'WORKOUT DATA {offset} {data[offset : offset + CHUNK].hex()}'
    yield 'WORKOUT COMMIT'


def upload(connection, slot, data):
    """Send the upload once; the device installs it only when COMMIT checks out."""
    reply = None
    for command in commands(slot, data):
        reply = connection.terminal_command(command)
        if reply['status'] != 'OK':
            raise RuntimeError(f'{command.split()[1]} refused: {reply["status"]} {reply["data"]}'.rstrip())
    return reply['data']


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('workout', type=Path, help='workout JSON file')
    parser.add_argument('--slot', type=int, default=0, help=f'upload slot 0..{UPLOAD_SLOTS - 1} (default 0)')
    parser.add_argument('--dry-run', action='store_true', help='print the commands without a device')
    add_port_argument(parser)
    args = parser.parse_args()
    data = encode(json.loads(args.workout.read_text()))
    if args.dry_run:
        print('\n'.join(commands(args.slot, data)))
        return
    port = selected_port(parser, args.port)
    log = ROOT / '.local/tests' / f'workout-load-{time.time_ns()}.log'
    log.parent.mkdir(parents=True, exist_ok=True)
    with UsbConnection(port, log) as connection:
        print(upload(connection, args.slot, data))


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        raise SystemExit(str(error)) from None
