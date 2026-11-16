import pytest

from cycling_devtools import lanes, workspace
from verify_fixture import IDENTITY, commit, git


@pytest.fixture
def checkout(tmp_path, monkeypatch):
    for name, value in IDENTITY.items():
        monkeypatch.setenv(name, value)
    monkeypatch.setenv('GATE_RUNS', str(tmp_path / 'runs.log'))
    origin = tmp_path / 'origin.git'
    git('init', '-q', '--bare', '-b', 'main', str(origin), cwd=tmp_path)
    root = tmp_path / 'work'
    git('clone', '-q', str(origin), str(root), cwd=tmp_path)
    git('checkout', '-q', '-b', 'main', cwd=root)
    commit(root, 'Start', **{'.gitignore': '/.local/\n', 'src/a.txt': 'a\n'})
    git('push', '-q', '-u', 'origin', 'main', cwd=root)

    probe = lanes.Gate('probe', 'echo ran >> "$GATE_RUNS"; test ! -e broken', test=True)
    monkeypatch.setattr(workspace, 'ROOT', root)
    monkeypatch.setattr(workspace, 'main_checkout', lambda: root)
    monkeypatch.setattr(lanes, 'LANES', ((probe,),))
    monkeypatch.setattr(lanes, 'ALONE', ())
    monkeypatch.setattr(lanes, 'EVERY', frozenset({'probe'}))
    monkeypatch.setattr(lanes, 'RULES', (('docs/', lanes.NONE),))
    monkeypatch.setattr(lanes, 'firmware_crates', lambda root: ())
    return root
