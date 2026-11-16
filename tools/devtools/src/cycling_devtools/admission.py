import contextlib
import fcntl
import os
import time

from cycling_devtools import clean, workspace

CORES_PER_RUN = 10
MIN_FREE_GB = 20
POLL_SECONDS = 1.0


def slots():
    return max(1, (os.cpu_count() or 1) // CORES_PER_RUN)


def free_slot(pool):
    for number in range(slots()):
        handle = (pool / f'slot-{number}').open('a')
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            handle.close()
        else:
            return handle
    return None


def require_disk():
    if (free := clean.free_gb(workspace.ROOT)) < MIN_FREE_GB:
        raise SystemExit(f'{free:.0f} GB free, under {MIN_FREE_GB} GB: free disk first (cycling clean --apply)')


@contextlib.contextmanager
def admitted():
    """Holds one of the machine's slots; yields the seconds spent waiting for it."""
    require_disk()
    pool = workspace.main_checkout() / '.local' / 'admission'
    pool.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    while (slot := free_slot(pool)) is None:
        time.sleep(POLL_SECONDS)
    with slot:
        # The runs ahead in the queue may have filled the disk.
        require_disk()
        yield time.monotonic() - started
