"""The tree a verify certifies: its hash, a watch for edits while the gates run, and gate results cached under it.

A tree hash covers tracked and untracked files that are not ignored, so a result
cannot certify a file the commit lacks. Gates run in the worktree itself, so a
Watch records the tree when they start and names every file that differs later.
"""

import contextlib
import fcntl
import hashlib
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

from cycling_devtools import workspace

RESULTS_KEPT_DAYS = 30


def git(*args, cwd=None, env=None):
    return subprocess.run(
        ['git', *args],
        cwd=cwd or workspace.ROOT,
        env=env and os.environ | env,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def results_dir():
    return workspace.main_checkout() / '.local' / 'verify' / 'results'


def tree_hash(root):
    """The tree of root as it is now. Nothing in the worktree or its index changes."""
    index = Path(git('rev-parse', '--path-format=absolute', '--git-path', 'index', cwd=root))
    with tempfile.TemporaryDirectory(dir=index.parent) as scratch:
        env = {'GIT_INDEX_FILE': str(Path(scratch) / 'index')}
        if index.exists():
            # The copy keeps the stat cache, so only changed files are hashed.
            Path(env['GIT_INDEX_FILE']).write_bytes(index.read_bytes())
        git('add', '-A', cwd=root, env=env)
        return git('write-tree', cwd=root, env=env)


def stamps(root):
    """Change time and size of every file the tree hash covers."""
    found = {}
    for name in filter(None, git('ls-files', '-z', '--cached', '--others', '--exclude-standard', cwd=root).split('\0')):
        with contextlib.suppress(OSError):
            status = os.lstat(root / name)
            found[name] = (status.st_ctime_ns, status.st_size)
    return found


class Watch:
    """The tree of root as it stood when the watch began."""

    def __init__(self, root):
        self.root = root
        # Stamps first: an edit between the two shows as a changed stamp.
        self.stamps = stamps(root)
        self.tree = tree_hash(root)

    def changed(self):
        """Files that differ now. An edit that was undone still changed its file's change time."""
        now = stamps(self.root)
        moved = {name for name in self.stamps.keys() | now.keys() if self.stamps.get(name) != now.get(name)}
        moved.update(git('diff', '--name-only', '--no-renames', self.tree, tree_hash(self.root)).splitlines())
        return sorted(moved)


@contextlib.contextmanager
def locked(path):
    """Hold an exclusive lock on path, waiting for whoever has it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a') as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def toolchain():
    """Identity of every tool the dev shell provides: its store paths are content-addressed."""
    stores = [entry for entry in os.environ.get('PATH', '').split(':') if entry.startswith('/nix/store/')]
    stores.append(os.environ.get('CYCLING_ESP_TOOLCHAIN', ''))
    return hashlib.sha256('\n'.join(stores).encode()).hexdigest()[:16]


def result_key(tree, *revision):
    """Key of one gate's result: the tree, the toolchain, and the gate's own definition and inputs."""
    return hashlib.sha256(json.dumps([tree, toolchain(), *revision]).encode()).hexdigest()[:32]


def cached_pass(key):
    """Seconds the gate took when it passed under this key, or None."""
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
