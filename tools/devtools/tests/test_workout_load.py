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
    header = [*b'VW', 2, 0, 2, *b'AB', 1, 1]
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
    assert data[:3] == b'VW\x02'
    assert len(data) <= 5 + 20 + 2 + 12 * 35 + 8 * 3 + 4


class FakeConnection:
    def __init__(self, listed):
        self.sent, self.listed = [], listed

    def terminal_command(self, command):
        self.sent.append(command)
        return {'status': 'OK', 'data': self.listed}


class FakeTransfer:
    def __init__(self):
        self.calls = []

    def call(self, op, *args, **kwargs):
        self.calls.append(op)


def fake_volumes(monkeypatch, transfer, put):
    monkeypatch.setattr(load.volumes, 'open_session', lambda connection: transfer)
    monkeypatch.setattr(load.volumes, 'files_put', put)


WORKOUT = load.encode({'name': 'Over unders', 'steps': [{'intensity': 'work', 'minutes': 1}]})


def test_upload_puts_todays_workout_on_files_and_has_vana_read_it(monkeypatch, tmp_path):
    transfer, put = FakeTransfer(), []

    def files_put(session, name, path):
        put.append((session, name, path.read_bytes()))
        return {'name': name, 'length': len(WORKOUT), 'crc32': f'{zlib.crc32(WORKOUT):08x}'}

    fake_volumes(monkeypatch, transfer, files_put)
    connection = FakeConnection('workout=OVER_UNDERS minutes=1 steps=1 files=ok')
    entry = load.upload(connection, WORKOUT, tmp_path)
    assert put == [(transfer, 'workouts/today.vw', WORKOUT)]
    assert transfer.calls == ['end']
    assert connection.sent == ['WORKOUT LOAD']
    assert entry['crc32'] == f'{zlib.crc32(WORKOUT):08x}'


def test_a_failed_put_still_ends_the_session_and_reads_nothing(monkeypatch, tmp_path):
    transfer = FakeTransfer()

    def files_put(session, name, path):
        raise RuntimeError('file readback differs')

    fake_volumes(monkeypatch, transfer, files_put)
    connection = FakeConnection('')
    with pytest.raises(RuntimeError, match='readback'):
        load.upload(connection, WORKOUT, tmp_path)
    assert transfer.calls == ['end']
    assert connection.sent == []


@pytest.mark.parametrize(
    ('name', 'reply'),
    [
        # VANA drops a trailing space.
        ('Tempo ', 'workout=TEMPO minutes=1 steps=1 files=ok'),
        # VANA cuts a name wider than its screens hold, and the space the cut
        # leaves at the end.
        ('WWWWWWWWWWWW WWWWWWW', 'workout=WWWWWWWWWWWW minutes=1 steps=1 files=ok'),
        ('MMMMMMMMMMMMMMMMMMMM', 'workout=MMMMMMMMMMMMMMMM minutes=1 steps=1 files=ok'),
    ],
)
def test_a_name_vana_kept_as_it_keeps_names_reads_back(monkeypatch, tmp_path, name, reply):
    data = load.encode({'name': name, 'steps': [{'intensity': 'work', 'minutes': 1}]})
    fake_volumes(monkeypatch, FakeTransfer(), lambda session, file, path: {'name': file})
    entry = load.upload(FakeConnection(reply), data, tmp_path)
    assert entry['today'] == reply


@pytest.mark.parametrize(
    'reply',
    [
        'workout=SWEET_SPOT minutes=1 steps=1 files=ok',
        'workout=TEMPO_ minutes=1 steps=1 files=ok',
        'workout= minutes=1 steps=1 files=ok',
    ],
)
def test_a_name_vana_did_not_keep_from_this_one_is_reported(monkeypatch, tmp_path, reply):
    data = load.encode({'name': 'Tempo hard ', 'steps': [{'intensity': 'work', 'minutes': 1}]})
    fake_volumes(monkeypatch, FakeTransfer(), lambda session, file, path: {'name': file})
    with pytest.raises(RuntimeError, match='did not read TEMPO_HARD'):
        load.upload(FakeConnection(reply), data, tmp_path)


def test_a_workout_vana_does_not_read_is_reported(monkeypatch, tmp_path):
    fake_volumes(monkeypatch, FakeTransfer(), lambda session, name, path: {'name': name})
    with pytest.raises(RuntimeError, match='did not read'):
        load.upload(FakeConnection('workout=none files=decode'), WORKOUT, tmp_path)
