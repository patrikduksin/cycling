import json
import struct
import zlib

import pytest

from cycling_devtools.workouts import load
from cycling_devtools.workspace import ROOT

EXAMPLES = sorted((ROOT / 'tools/devtools/workouts').glob('*.json'))


def test_layout_matches_the_device_codec_pin():
    # The same workout and bytes as `layout_is_pinned` in apps/vana/src/workout/codec.rs.
    data = load.encode(
        {
            'name': 'ab',
            'steps': [{'repeat': 2, 'steps': [{'intensity': 'work', 'minutes': 5, 'ftp': [90, 95], 'note': 'go'}]}],
        }
    )
    header = [*b'VW', 1, 0, 2, *b'AB', 1, 1]
    step = [1, 0, 0x2C, 0x01, 0, 0, 1, 90, 0, 95, 0, 0, 0, 0, 2, *b'GO']
    body = bytes([*header, *step, 0, 1, 2])
    assert data == body + struct.pack('<I', zlib.crc32(body))


def test_lengths_targets_and_tests_encode_their_kinds():
    data = load.encode(
        {
            'name': 'Ramp',
            'test': 'ramp',
            'steps': [
                {'intensity': 'test', 'open': True, 'ramp': {'start': 100, 'step': 20, 'every': 60}},
                {'intensity': 'cooldown', 'km': 2.5, 'heart_zone': 1},
                {'intensity': 'recovery', 'seconds': 30},
            ],
        }
    )
    assert data[3] == 2
    steps = data[5 + 4 + 2 :]
    assert struct.unpack_from('<BBIB3HBB', steps, 0) == (4, 2, 0, 6, 100, 20, 60, 0, 0)
    assert struct.unpack_from('<BBIB3HBB', steps, 15) == (3, 1, 2500, 3, 1, 0, 0, 0, 0)
    assert struct.unpack_from('<BBIB3HBB', steps, 30) == (2, 0, 30, 0, 0, 0, 0, 0, 0)


@pytest.mark.parametrize(
    'workout',
    [
        {'name': 'X', 'steps': []},
        {'name': 'X', 'steps': [{'intensity': 'work'}]},
        {'name': 'X', 'steps': [{'intensity': 'work', 'minutes': 1, 'seconds': 3}]},
        {'name': 'X', 'steps': [{'intensity': 'work', 'minutes': 1, 'ftp': [90, 80]}]},
        {'name': 'X', 'steps': [{'intensity': 'work', 'minutes': 1, 'ftp': [90, 95], 'watts': [1, 2]}]},
        {'name': 'X', 'steps': [{'intensity': 'sprint', 'minutes': 1}]},
        {'name': 'X', 'steps': [{'intensity': 'work', 'metres': 50}]},
        {'name': 'X', 'steps': [{'intensity': 'work', 'minutes': 1, 'colour': 'red'}]},
        {'name': 'A NAME FAR TOO LONG TO FIT', 'steps': [{'intensity': 'work', 'minutes': 1}]},
        {'name': 'X', 'test': 'sprint', 'steps': [{'intensity': 'work', 'minutes': 1}]},
        {'name': 'X', 'steps': [{'intensity': 'work', 'minutes': 1}] * 13},
    ],
)
def test_invalid_workouts_are_refused_on_the_host(workout):
    with pytest.raises(ValueError):
        load.encode(workout)


@pytest.mark.parametrize('path', EXAMPLES, ids=lambda path: path.stem)
def test_examples_encode(path):
    data = load.encode(json.loads(path.read_text()))
    assert data[:3] == b'VW\x01'
    assert len(data) <= 5 + 20 + 2 + 12 * 35 + 8 * 3 + 4


class FakeConnection:
    def __init__(self, refuse=None):
        self.sent, self.refuse = [], refuse

    def terminal_command(self, command):
        self.sent.append(command)
        if self.refuse and command.startswith(self.refuse):
            return {'status': 'CHECKSUM', 'data': ''}
        return {'status': 'OK', 'data': 'slot=1 name=X' if command == 'WORKOUT COMMIT' else ''}


def test_upload_sends_begin_chunks_and_commit():
    data = bytes(range(100))
    connection = FakeConnection()
    assert load.upload(connection, 1, data) == 'slot=1 name=X'
    assert connection.sent == [
        f'WORKOUT BEGIN 1 100 {zlib.crc32(data):08x}',
        f'WORKOUT DATA 0 {data[:48].hex()}',
        f'WORKOUT DATA 48 {data[48:96].hex()}',
        f'WORKOUT DATA 96 {data[96:].hex()}',
        'WORKOUT COMMIT',
    ]
    # Every command fits the console's domain-command limit.
    assert max(len(command) for command in connection.sent) <= 128


def test_a_refused_commit_is_reported_and_not_retried():
    connection = FakeConnection(refuse='WORKOUT COMMIT')
    with pytest.raises(RuntimeError, match='COMMIT refused: CHECKSUM'):
        load.upload(connection, 0, b'VW')
    assert connection.sent.count('WORKOUT COMMIT') == 1


def test_slots_outside_the_store_are_refused():
    with pytest.raises(ValueError):
        list(load.commands(4, b'VW'))
