import struct
import zlib

import pytest

from cycling_devtools.course import decode_course, decode_turns, main


def sealed(body):
    return body + struct.pack('<I', zlib.crc32(body))


def course_file(points=((-334500000, -706600000), (-334510000, -706600000)), name=b'LOOP'):
    header = b'VCRS' + bytes([2, 34, 8, len(name)]) + struct.pack('<H', len(points)) + name.ljust(24, b'\0')
    return sealed(header + b''.join(struct.pack('<ii', *p) for p in points))


def turns_file(key, turns, full=False, record=12):
    header = b'VTRN' + bytes([2, 16, record, int(full)]) + struct.pack('<IHH', key, len(turns), 0)
    rows = b''.join(struct.pack('<IIBB', *turn).ljust(record, b'\0') for turn in turns)
    return sealed(header + rows)


def test_a_course_and_its_turns_decode():
    course = decode_course(course_file())
    assert course.name == 'LOOP' and len(course.points) == 2
    stored = decode_turns(turns_file(course.key, [(400_000, 415_000, 2, 2), (900_000, 960_000, 5, 0)]))
    assert stored.key == course.key and not stored.full
    assert [(t.start_mm, t.kind, t.cornered) for t in stored.turns] == [
        (400_000, 'RIGHT', True),
        (900_000, 'U-TURN', False),
    ]


def test_the_key_is_the_crc_that_ends_the_course_file():
    stored = course_file()
    assert decode_course(stored).key == struct.unpack('<I', stored[-4:])[0]


def test_another_version_or_record_length_is_refused():
    stored = course_file()
    with pytest.raises(ValueError):
        decode_course(sealed(stored[:4] + bytes([1]) + stored[5:-4]))
    turns = turns_file(7, [(1, 2, 1, 1)])
    with pytest.raises(ValueError):
        decode_turns(sealed(turns[:4] + bytes([1]) + turns[5:-4]))
    with pytest.raises(ValueError):
        decode_turns(turns_file(7, [(1, 2, 1, 1)], record=16))


@pytest.mark.parametrize(
    'damage',
    [
        lambda b: b[:-1],
        lambda b: b + b'\0',
        lambda b: b[:20] + bytes([b[20] ^ 1]) + b[21:],
        lambda b: sealed(b[:24] + bytes([9]) + b[25:-4]),
        lambda b: sealed(b[:16] + struct.pack('<I', 5_000_000) + b[20:-4]),
    ],
)
def test_a_damaged_turns_file_is_refused(damage):
    good = turns_file(1, [(400_000, 415_000, 2, 2), (900_000, 960_000, 5, 0)])
    decode_turns(good)
    with pytest.raises(ValueError):
        decode_turns(damage(good))


def test_the_tool_says_whose_turns_they_are(tmp_path, capsys):
    course = course_file()
    (tmp_path / 'course.dat').write_bytes(course)
    (tmp_path / 'course-turns.dat').write_bytes(turns_file(1, [(400_000, 415_000, 2, 2)]))
    assert main([str(tmp_path / 'course.dat'), str(tmp_path / 'course-turns.dat')]) == 0
    assert 'scanned instead' in capsys.readouterr().out
