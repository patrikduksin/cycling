import json
import threading
import time

import pytest

from cycling_devtools import deep_tier, forge, lanes, verify
from verify_fixture import commit, git

FAKE_VERIFY_DEEP = """
git rev-parse HEAD >> "$GATE_RUNS"
while [ ! -e "$DEEP_GO" ]; do sleep 0.02; done
if [ -e broken ]; then echo 'FAIL probe (0 s)'; exit 1; fi
echo 'ok   probe (0 s)'
"""
PROBE = lanes.Gate('probe', 'echo "bisect $(git rev-parse HEAD)" >> "$GATE_RUNS"; test ! -e broken')


class Issues(forge.Issues):
    def __init__(self):
        self.marks, self.created = set(), []

    def filed(self, mark):
        return mark in self.marks

    def file(self, title, body):
        self.created.append((title, body))
        self.marks.add(body.split('\n', 1)[0])


@pytest.fixture
def tier(checkout, monkeypatch, tmp_path):
    monkeypatch.setattr(deep_tier, 'COMMAND', FAKE_VERIFY_DEEP)
    monkeypatch.setattr(lanes, 'DEEP', (PROBE,))
    monkeypatch.setenv('DEEP_GO', str(tmp_path / 'go'))
    (tmp_path / 'go').touch()
    return checkout


def land(root, message, **files):
    head = commit(root, message, **files)
    git('push', '-q', 'origin', 'main', cwd=root)
    return head


def runs(root):
    log = root.parent / 'runs.log'
    return log.read_text().splitlines() if log.exists() else []


def state(root):
    return json.loads((root / '.local' / 'nightly' / 'deep.json').read_text())


def test_a_failure_planted_two_commits_back_is_named_by_an_unattended_run(tier):
    green = git('rev-parse', 'HEAD', cwd=tier)
    issues = Issues()
    assert deep_tier.request(issues) == [deep_tier.Run(green)]

    land(tier, 'One', **{'src/one.txt': '1\n'})
    planted = land(tier, 'Plant the bug', **{'broken': 'yes\n'})
    land(tier, 'Three', **{'src/three.txt': '3\n'})
    head = land(tier, 'Four', **{'src/four.txt': '4\n'})

    assert deep_tier.request(issues) == [deep_tier.Run(head, deep_tier.GateFailed('probe'), planted)]
    assert state(tier) == {'green': green, 'last': {'commit': head, 'failed': 'probe', 'first_bad': planted}}
    [(title, body)] = issues.created
    assert title == f'Tier 2: probe fails from {planted[:12]}'
    assert f'names {planted} (Plant the bug) as the first bad commit' in body
    assert 'FAIL probe' in body
    assert f'bisect {green}' in runs(tier), 'the bisect proves the last green commit passes the gate alone'
    assert git('rev-parse', 'HEAD', cwd=tier / '.local' / 'merge' / 'deep') == green, 'the bisect was reset'

    assert deep_tier.request(issues) == [deep_tier.Run(head, deep_tier.GateFailed('probe'), planted)]
    assert len(issues.created) == 1, 'the same first bad commit is filed once'


def test_requests_during_a_run_become_one_more_run_on_the_newest_main(tier, tmp_path):
    (tmp_path / 'go').unlink()
    first = git('rev-parse', 'HEAD', cwd=tier)
    done = []
    running = threading.Thread(target=lambda: done.extend(deep_tier.request(Issues())))
    running.start()
    deadline = time.monotonic() + 30
    while runs(tier) != [first]:
        assert time.monotonic() < deadline, 'the first run never started'
        time.sleep(0.02)

    land(tier, 'Merged during the run', **{'src/two.txt': '2\n'})
    assert deep_tier.request(Issues()) == []
    newest = land(tier, 'Merged during the run too', **{'src/three.txt': '3\n'})
    assert deep_tier.request(Issues()) == []

    (tmp_path / 'go').touch()
    running.join(30)
    assert done == [deep_tier.Run(first), deep_tier.Run(newest)]
    assert runs(tier) == [first, newest]
    assert state(tier)['green'] == newest


def test_a_gate_that_fails_on_the_last_green_commit_too_blames_no_commit(tier, monkeypatch):
    green = git('rev-parse', 'HEAD', cwd=tier)
    issues = Issues()
    deep_tier.request(issues)
    head = land(tier, 'Unrelated', **{'src/one.txt': '1\n'})
    monkeypatch.setattr(lanes, 'DEEP', (lanes.Gate('probe', 'false'),))
    monkeypatch.setattr(deep_tier, 'COMMAND', "echo 'FAIL probe (0 s)'; exit 1")

    assert deep_tier.request(issues) == [deep_tier.Run(head, deep_tier.GateFailed('probe'))]
    [(title, body)] = issues.created
    assert title == f'Tier 2: probe fails on main at {head[:12]}'
    assert f'fails on {green}, the last green commit, too' in body
    assert state(tier)['green'] == green


def test_the_failed_gate_is_read_from_what_verify_deep_prints(checkout, monkeypatch, capsys):
    monkeypatch.setattr(lanes, 'DEEP', (lanes.Gate('first', 'true'), PROBE))
    commit(checkout, 'Plant the bug', **{'broken': 'yes\n'})
    outcome = verify.verify(verify.Request(checkout, 'HEAD', deep=True))
    assert not outcome.ok
    assert deep_tier.FAILED.search(capsys.readouterr().out)[1] == 'probe'


def test_a_deep_verify_stopped_by_a_changed_tree_runs_again_and_blames_no_commit(tier, monkeypatch, tmp_path):
    stops_once = f"""
git rev-parse HEAD >> "$GATE_RUNS"
if [ -e {tmp_path}/stop ]; then rm {tmp_path}/stop; echo 'cycling verify stopped: the tree changed'; exit 1; fi
echo 'ok   probe (0 s)'
"""
    monkeypatch.setattr(deep_tier, 'COMMAND', stops_once)
    issues = Issues()
    deep_tier.request(issues)
    head = land(tier, 'Two', **{'src/two.txt': '2\n'})
    (tmp_path / 'stop').touch()

    assert deep_tier.request(issues) == [deep_tier.Run(head, deep_tier.Stopped()), deep_tier.Run(head)]
    assert issues.created == []
    assert state(tier)['green'] == head


def test_a_gate_that_passes_when_run_again_on_the_head_files_nothing(tier, monkeypatch, tmp_path):
    green = git('rev-parse', 'HEAD', cwd=tier)
    issues = Issues()
    deep_tier.request(issues)
    head = land(tier, 'Two', **{'src/two.txt': '2\n'})
    monkeypatch.setattr(deep_tier, 'COMMAND', "echo 'FAIL probe (0 s)'; exit 1")
    monkeypatch.setattr(lanes, 'DEEP', (lanes.Gate('probe', 'true'),))

    assert deep_tier.request(issues) == [deep_tier.Run(head, deep_tier.GateFailed('probe'))]
    assert issues.created == []
    assert state(tier)['green'] == green


def test_a_run_that_dies_leaves_its_request_for_the_next(tier, monkeypatch):
    def dies(*_):
        raise RuntimeError('fetch failed')

    monkeypatch.setattr(deep_tier, 'run_newest', dies)
    with pytest.raises(RuntimeError):
        deep_tier.request(Issues())
    assert (tier / '.local' / 'nightly' / 'deep.pending').exists()
