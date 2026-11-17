import contextlib
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from cycling_devtools import workspace

RESULTS_KEPT_DAYS = 30
KEYED_ENV_PREFIXES = ('CYCLING_', 'CARGO_BUILD_', 'CARGO_PROFILE_', 'CARGO_TARGET_')
KEYED_ENV_NAMES = frozenset(
    {'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTDOCFLAGS', 'NEXTEST_PROFILE'}
)


def git(*args, cwd=None, env=None):
    return subprocess.run(
        ['git', *args],
        cwd=cwd or workspace.ROOT,
        env=env and os.environ | env,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def merge_base(commit):
    for main in ('origin/main', 'main'):
        done = subprocess.run(['git', 'merge-base', main, commit], cwd=workspace.ROOT, capture_output=True, text=True)
        if done.returncode == 0:
            return done.stdout.strip()
    raise SystemExit('cycling: no origin/main or main to compare with')


def changed_paths(base, tree_id):
    return git('diff', '--name-only', '--no-renames', base, tree_id).splitlines()


def results_dir():
    return workspace.main_checkout() / '.local' / 'verify' / 'results'


def tree_hash(root):
    index = Path(git('rev-parse', '--path-format=absolute', '--git-path', 'index', cwd=root))
    with tempfile.TemporaryDirectory(dir=index.parent) as scratch:
        env = {'GIT_INDEX_FILE': str(Path(scratch) / 'index')}
        if index.exists():
            # The copy keeps the stat cache, so only changed files are hashed, and the index's own
            # time, which git needs to tell an edit made in the second the index was written.
            shutil.copy2(index, env['GIT_INDEX_FILE'])
        git('add', '-A', cwd=root, env=env)
        return git('write-tree', cwd=root, env=env)


def stamps(root):
    found = {}
    for name in filter(None, git('ls-files', '-z', '--cached', '--others', '--exclude-standard', cwd=root).split('\0')):
        with contextlib.suppress(OSError):
            status = os.lstat(root / name)
            found[name] = (status.st_ctime_ns, status.st_size)
    return found


class Watch:
    def __init__(self, root):
        self.root = root
        self.stamps = stamps(root)
        self.tree = tree_hash(root)

    def changed(self):
        """An edit that was undone still changed its file's change time."""
        now = stamps(self.root)
        moved = {name for name in self.stamps.keys() | now.keys() if self.stamps.get(name) != now.get(name)}
        moved.update(changed_paths(self.tree, tree_hash(self.root)))
        return sorted(moved)


def toolchain():
    return [entry for entry in os.environ.get('PATH', '').split(':') if entry.startswith('/nix/store/')]


def keyed_env(gate_env):
    ambient = {
        name: value
        for name, value in os.environ.items()
        if name in KEYED_ENV_NAMES or (name.startswith(KEYED_ENV_PREFIXES) and name != 'CARGO_TARGET_DIR')
    }
    return sorted((ambient | dict(gate_env)).items())


def result_key(tree, gate, command, gate_env):
    inputs = [tree, toolchain(), gate, command, keyed_env(gate_env)]
    return hashlib.sha256(json.dumps(inputs).encode()).hexdigest()[:32]


def cached_pass(key):
    try:
        return json.loads((results_dir() / f'{key}.json').read_text())['seconds']
    except OSError, ValueError, KeyError:
        return None


def record_pass(key, gate, tree, seconds):
    directory = results_dir()
    directory.mkdir(parents=True, exist_ok=True)
    entry = {'gate': gate, 'tree': tree, 'seconds': seconds, 'at': time.time()}
    with tempfile.NamedTemporaryFile('w', dir=directory, prefix='.', delete=False) as staging:
        json.dump(entry, staging)
    os.replace(staging.name, directory / f'{key}.json')
    expired = time.time() - RESULTS_KEPT_DAYS * 86400
    for old in directory.glob('*.json'):
        with contextlib.suppress(OSError):
            if old.stat().st_mtime < expired:
                old.unlink()
