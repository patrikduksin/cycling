"""Admission to heavy runs: one pool of slots for every worktree on the machine.

A verify with gates to run keeps about ten cores busy, so more of them at once
than the cores carry only make each other slower. Each takes a slot first and
waits when none is free. A slot is a lock file in the primary checkout, so it
frees itself when its holder dies.
"""

import contextlib
import fcntl
import os
import time

from cycling_devtools import clean, workspace

CORES_PER_RUN = 10
# A cold build of every target dir writes about 9 GB.
MIN_FREE_GB = 20
POLL_SECONDS = 1.0


def slots():
    return max(1, (os.cpu_count() or 1) // CORES_PER_RUN)


def free_slot(pool):
    """An open, locked slot file, or None when every slot is held."""
    for number in range(slots()):
        handle = (pool / f'slot-{number}').open('a')
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            handle.close()
        else:
            return handle
    return None


@contextlib.contextmanager
def admitted():
    """Hold a slot while the block runs; yields the seconds spent waiting for it."""
    if (free := clean.free_gb(workspace.ROOT)) < MIN_FREE_GB:
        raise SystemExit(f'{free:.0f} GB free, under {MIN_FREE_GB} GB: free disk first (cycling clean --apply)')
    pool = workspace.main_checkout() / '.local' / 'admission'
    pool.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    while (slot := free_slot(pool)) is None:
        time.sleep(POLL_SECONDS)
    with slot:
        yield time.monotonic() - started
