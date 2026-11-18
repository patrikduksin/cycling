import os

import pytest

from cycling_devtools import corpus


def trace(directory, name, version=7, age=0):
    path = directory / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b'\x40\x00' + bytes([version]) + bytes(20))
    os.utime(path, (1_000_000 - age, 1_000_000 - age))
    return path


@pytest.fixture
def rides(tmp_path, monkeypatch):
    directory = tmp_path / 'ride-trace'
    directory.mkdir()
    monkeypatch.setattr(corpus, 'corpus', lambda root: [directory, directory.parent / 'absent'])
    monkeypatch.setattr(corpus.upgrade, 'current_format', lambda root: 7)
    return directory


def replies(by_name):
    return lambda trace, out, root: by_name[trace.name]


def test_only_this_formats_traces_replay_newest_first_and_never_a_replays_own(rides):
    trace(rides, 'old.trace', age=30)
    trace(rides, 'new.trace', age=10)
    trace(rides, 'mid/ride.trace', age=20)
    trace(rides, 'v6.trace', version=6)
    trace(rides, 'replay-x/replayed.trace')
    seen = []

    def replay(trace, out, root):
        seen.append(trace.name)
        return {'stopped': None}

    _, _, results = corpus.run(last=2, replay=replay, jobs=1)
    assert seen == ['new.trace', 'ride.trace']
    assert [result.verdict for result in results] == ['passed', 'passed']


def test_a_stop_is_printed_and_only_a_panic_or_broken_invariant_fails(rides, monkeypatch, capsys):
    trace(rides, 'stops.trace', age=1)
    trace(rides, 'breaks.trace', age=2)
    trace(rides, 'missing-pack.trace', age=3)
    stop = {'kind': 'Unrecorded', 'at_ms': 36_807, 'detail': 'left the recorded world at 00:36.807: Sound::play(00)'}
    broke = {'kind': 'Panicked', 'at_ms': 5_000, 'detail': 'VANA panicked: invariant'}
    replay = replies(
        {
            'stops.trace': {'stopped': stop},
            'breaks.trace': {'stopped': broke},
            'missing-pack.trace': {'error': 'no TERRAIN pack ab12'},
        }
    )
    monkeypatch.setattr(corpus, 'replay_json', replay)
    assert corpus.main([]) == 1
    out = capsys.readouterr().out
    assert 'stopped      ' in out and 'Sound::play(00)' in out
    assert 'broken       ' in out and 'not replayed' in out
    assert 'corpus: 3 replayed, 1 broken' in out
    monkeypatch.setattr(corpus, 'replay_json', replies({'stops.trace': {'stopped': stop}}))
    assert corpus.main(['--last', '1']) == 0


def test_a_checkout_without_the_corpus_skips_and_says_so(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(corpus, 'corpus', lambda root: [tmp_path / 'absent'])
    monkeypatch.setattr(corpus.upgrade, 'current_format', lambda root: 7)
    assert corpus.main([]) == 0
    assert 'corpus: no format-7 trace in' in capsys.readouterr().out


def test_the_listing_names_the_traces_a_run_replays_and_the_private_fixtures(rides, tmp_path, monkeypatch, capsys):
    for age in range(12):
        trace(rides, f'ride-{age:02}.trace', age=age)
    trace(rides, 'v6.trace', version=6)
    for last in ([], ['--all']):
        replayed = []
        monkeypatch.setattr(corpus, 'replay_json', lambda trace, out, root: replayed.append(str(trace)) or {})
        corpus.main(last)
        capsys.readouterr()
        assert corpus.main([*last, '--list']) == 0
        assert sorted(capsys.readouterr().out.split()) == sorted(replayed)
        assert len(replayed) == (12 if last else 10)
    fixture = tmp_path / '.local' / 'fixtures' / 'paused'
    fixture.mkdir(parents=True)
    for name in ('slice.trace', 'expect.toml', '.slice.trace.new'):
        (fixture / name).write_text('')
    monkeypatch.setattr(corpus.workspace, 'main_checkout', lambda root: tmp_path)
    assert corpus.main(['--list-fixtures']) == 0
    assert capsys.readouterr().out.split() == [str(fixture / 'expect.toml'), str(fixture / 'slice.trace')]
