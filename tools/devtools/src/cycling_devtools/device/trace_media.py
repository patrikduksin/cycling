"""Pull a ride trace and the moments the rider marked in it."""

import json
import shlex
import subprocess
import time
from pathlib import Path

from cycling_devtools.cli import TRACE
from cycling_devtools.device import volumes
from cycling_devtools.workspace import ROOT


def trace_tool(*args):
    """Run `cycling trace` and return what it printed."""
    command = [*shlex.split(TRACE), 'trace', *map(str, args)]
    done = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if done.returncode != 0:
        raise RuntimeError(f'cycling trace {args[0]}: {done.stderr.strip()}')
    return done.stdout


def marks_text(marks):
    if not marks:
        return 'marks: none'
    return '\n'.join(['marks:', *(f'  mark {mark["text"]}' for mark in marks)])


def session(connection, timing, name, work):
    """Run `work(transfer)` in one transfer session, the app paused, and note how long it took here and where its
    time went on the device."""
    started = time.monotonic()
    transfer = volumes.open_session(connection)
    try:
        return work(transfer)
    finally:
        spent = volumes.end(transfer)
        timing[name] = {'s': round(time.monotonic() - started, 3), **spent}


def pull(connection, name=None, directory=Path('.local/traces'), tool=trace_tool):
    """Pull the trace `name` (the newest by default) and list its marks."""
    started, timing = time.monotonic(), {}
    result = session(
        connection, timing, 'trace_session', lambda transfer: volumes.trace_pull(transfer, name, directory)
    )
    marks = json.loads(tool('find', Path(result['path']), 'mark', '--limit', 'all', '--json'))
    timing['total_s'] = round(time.monotonic() - started, 3)
    result.update(marks=marks['found'], timing=timing)
    return result
