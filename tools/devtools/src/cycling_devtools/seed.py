"""Start a fresh worktree's target dirs as a copy of a warm checkout's (#401).

Cargo keys dependencies by package id, not checkout path, so a copy brings
dependencies, build scripts and proc-macros built; the worktree's own crates
rebuild because its sources are newer than the copied outputs. Every task
that builds seeds the target dirs the worktree is missing, so worktrees made
by T3, by agent isolation or by hand all start warm.
"""

import fcntl
import shutil
import subprocess
import sys
import time
from contextlib import ExitStack, contextmanager

from cycling_devtools.workspace import ROOT, main_checkout

# Every target dir `cycling verify` builds in, relative to a checkout; target/
# holds clippy's and the screens base's dirs too.
SEEDED = ('target', 'vendor/trouble-host/target', '.local/firmware')
STAGING = '.local/seeding'


def candidates(main):
    # `cycling merge` builds every merged main in its checkout (#418).
    return [main / '.local/merge/verify', main]


def commit_time(checkout):
    out = subprocess.run(['git', 'log', '-1', '--format=%ct'], cwd=checkout, capture_output=True, text=True)
    return int(out.stdout) if out.returncode == 0 and out.stdout.strip() else None


def source(main=None):
    """The checkout to copy from: of those holding a seeded dir, the one on the newest commit."""
    best = None
    for checkout in candidates(main_checkout() if main is None else main):
        if not any((checkout / relative).is_dir() for relative in SEEDED):
            continue
        when = commit_time(checkout)
        if when is not None and (best is None or when > best[0]):
            best = (when, checkout)
    return best[1] if best else None


def try_lock(stack, path):
    handle = stack.enter_context(path.open('a'))
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return False
    return True


@contextmanager
def builds_held(directory, poll=1.0):
    """Hold every cargo lock in directory exclusively, as `cargo clean` does, so no build writes
    while it is copied. All or none: cargo takes a host and a target dir lock in its own order."""
    locks = sorted({p for depth in range(1, 4) for p in directory.glob('*/' * depth + '.cargo-lock')})
    said = False
    while True:
        with ExitStack() as stack:
            if all(try_lock(stack, path) for path in locks):
                yield
                return
        if not said:
            print(f'cycling: waiting for builds in {directory} to finish before copying it', file=sys.stderr)
            said = True
        time.sleep(poll)


def seed(root=None, origin=None):
    """Copy each seeded dir root (this checkout) lacks from origin (the source()); returns the dirs copied."""
    root = root or ROOT
    if all((root / relative).exists() for relative in SEEDED):
        return []
    origin = origin or source()
    if origin is None or origin.resolve() == root.resolve():
        return []
    staging = root / STAGING
    staging.mkdir(parents=True, exist_ok=True)
    copied = []
    with (staging / 'lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        for relative in SEEDED:
            destination, warm = root / relative, origin / relative
            if destination.exists() or not warm.is_dir():
                continue
            copy = staging / relative.replace('/', '-')
            shutil.rmtree(copy, ignore_errors=True)
            started = time.monotonic()
            with builds_held(warm):
                subprocess.run(['cp', '-a', '--reflink=auto', str(warm), str(copy)], check=True)
            destination.parent.mkdir(parents=True, exist_ok=True)
            try:
                copy.rename(destination)
            except OSError:
                shutil.rmtree(copy)
                continue
            print(f'cycling: seeded {relative} from {origin} in {time.monotonic() - started:.0f} s', file=sys.stderr)
            copied.append(relative)
    return copied
