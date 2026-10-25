"""Decode VANA's course files from the FILES volume: `course.dat` and the
turns stored beside it, `course-turns.dat` (docs/ride-recording.md, Course).

    python -m cycling_devtools.course course.dat [course-turns.dat]

A turns file names its course by its key, the CRC32 of the course encoded as
version 1 of `course.dat` (for a version 1 file, the CRC that ends it); turns
naming another course are not that course's, and the device scans for them.
"""

import argparse
import struct
import zlib
from dataclasses import dataclass
from pathlib import Path

from .little_endian import i32, u16, u32

KINDS = {0: None, 1: 'LEFT', 2: 'RIGHT', 3: 'SHARP LEFT', 4: 'SHARP RIGHT', 5: 'U-TURN'}


@dataclass
class Course:
    name: str
    points: list
    key: int


@dataclass
class Turn:
    start_mm: int
    end_mm: int
    kind: str | None
    twice: bool
    cornered: bool


@dataclass
class Turns:
    key: int
    full: bool
    turns: list


def _sealed(data):
    if len(data) < 4 or zlib.crc32(data[:-4]) != u32(data, len(data) - 4):
        raise ValueError('CRC does not match')
    return u32(data, len(data) - 4)


def decode_course(data):
    if len(data) < 38 or data[:4] != b'VCRS' or data[4] < 1:
        raise ValueError('not a course')
    header, point, name_len, count = data[5], data[6], data[7], u16(data, 8)
    if header < 34 or point < 8 or name_len > 24 or not 2 <= count <= 2048:
        raise ValueError('not a course')
    if header + count * point + 4 != len(data):
        raise ValueError('length does not match')
    _sealed(data)
    points = [(i32(data, header + k * point), i32(data, header + k * point + 4)) for k in range(count)]
    name = data[10 : 10 + name_len]
    return Course(name.decode('ascii'), points, version_1_key(name, points))


def version_1_key(name, points):
    """The CRC32 of the course encoded as version 1, before its CRC."""
    head = b'VCRS' + bytes([1, 34, 8, len(name)]) + struct.pack('<H', len(points)) + name.ljust(24, b'\0')
    return zlib.crc32(head + b''.join(struct.pack('<ii', *p) for p in points))


def decode_turns(data):
    if len(data) < 20 or data[:4] != b'VTRN' or data[4] < 1:
        raise ValueError('not a turns file')
    header, record, count = data[5], data[6], u16(data, 12)
    if header < 16 or record < 12 or count > 1024:
        raise ValueError('not a turns file')
    if header + count * record + 4 != len(data):
        raise ValueError('length does not match')
    _sealed(data)
    turns = []
    for k in range(count):
        at = header + k * record
        if data[at + 8] not in KINDS:
            raise ValueError(f'turn {k}: unknown kind {data[at + 8]}')
        flags = data[at + 9]
        turns.append(Turn(u32(data, at), u32(data, at + 4), KINDS[data[at + 8]], bool(flags & 1), bool(flags & 2)))
    for k, turn in enumerate(turns):
        before = turns[k - 1] if k else Turn(0, 0, None, False, False)
        if not before.start_mm <= turn.start_mm <= turn.end_mm or turn.end_mm < before.end_mm:
            raise ValueError(f'turn {k}: out of order')
    return Turns(u32(data, 8), bool(data[7] & 1), turns)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('course', type=Path)
    parser.add_argument('turns', type=Path, nargs='?')
    args = parser.parse_args(argv)
    course = decode_course(args.course.read_bytes())
    print(f'{course.name or "(unnamed)"}: {len(course.points)} points, key {course.key:08x}')
    if args.turns is None:
        return 0
    stored = decode_turns(args.turns.read_bytes())
    own = 'its own' if stored.key == course.key else f"another course's (key {stored.key:08x}): scanned instead"
    more = ', more than stored' if stored.full else ''
    print(f'{len(stored.turns)} turns{more}, {own}')
    for turn in stored.turns:
        if turn.kind:
            then = f' THEN {turn.kind}' if turn.twice else ''
            print(f'  {turn.start_mm / 1000:9.1f} m  {turn.kind}{then}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
