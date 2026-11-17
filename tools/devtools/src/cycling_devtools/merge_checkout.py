import contextlib
import fcntl

from cycling_devtools import workspace
from cycling_devtools.tree import git


def directory():
    return workspace.main_checkout() / '.local' / 'merge'


@contextlib.contextmanager
def locked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a') as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def checkout(name, commit):
    place = directory() / name
    if not (place / '.git').exists():
        place.parent.mkdir(parents=True, exist_ok=True)
        git(
            'worktree', 'add', '--quiet', '--detach', '--lock', '--reason', 'cycling merge', str(place), commit,
            cwd=workspace.main_checkout(),
        )  # fmt: skip
    git('checkout', '--quiet', '--force', '--detach', commit, cwd=place)
    git('clean', '-fdq', cwd=place)
    return place
