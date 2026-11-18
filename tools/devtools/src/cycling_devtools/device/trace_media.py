"""Pull a ride trace and the moments the rider marked in it, and check once that it replays faithfully under the
build that recorded it: a divergence there is a recording bug."""

import hashlib
import json
import shlex
import subprocess
import tempfile
import time
from pathlib import Path

from cycling_devtools import twin, workspace
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


def replay_faithful(binary, trace):
    """The faithful replay's JSON document, or {error}."""
    with tempfile.TemporaryDirectory(prefix='faithful-') as out:
        command = [str(binary), 'replay', str(trace), '--mode', 'faithful', '--to', '99999:00']
        command += ['--every', '99999:00', '--out', str(Path(out) / 'replay'), '--json']
        done = subprocess.run(command, capture_output=True, text=True)
    try:
        return json.loads(done.stdout)
    except json.JSONDecodeError:
        return {'error': done.stderr.strip() or f'{binary} printed no JSON'}


def faithful_twin(build, root):
    """The twin of the archived build `build`, which must replay faithfully."""
    return twin.binary_for_build(build, root=root, needs='faithful')


def faithful(
    trace,
    build,
    *,
    root=ROOT,
    twin_for=faithful_twin,
    check=twin.faithful_to,
    replay=replay_faithful,
):
    """{ok, ended, exclusions} of `trace` replayed faithfully under the twin of `build`, the build that recorded
    it, or {skipped: why}. A replay that leaves the recorded world at boot is a failed verdict. A verdict is cached
    by the trace's SHA-256 and the build."""
    digest = hashlib.sha256(Path(trace).read_bytes()).hexdigest()
    cached = workspace.main_checkout(root) / '.local' / 'traces' / '.faithful' / f'{digest}-{build}.json'
    if cached.exists():
        return json.loads(cached.read_text())
    try:
        binary = twin_for(build, root=root)
    except (twin.NotArchived, twin.Incapable) as error:
        return {'skipped': str(error)}
    except (OSError, subprocess.CalledProcessError, RuntimeError) as error:
        return {'skipped': f'the twin of build {build[:12]} did not build: {error}'}
    if why := check(binary, build, root=root):
        return {'skipped': why}
    replayed = replay(binary, trace)
    if 'left' in replayed:
        verdict = {'ok': False, 'ended': replayed['left'], 'exclusions': []}
    elif 'no_pack' in replayed:
        return {'skipped': f'no TERRAIN pack {replayed["no_pack"]}'}
    elif 'error' in replayed:
        return {'skipped': f'the twin of build {build[:12]} did not replay it: {replayed["error"]}'}
    else:
        divergence, proved = replayed['divergence'], replayed['faithful']
        ok = divergence['ok']
        verdict = {
            'ok': ok,
            'ended': proved['ended'] if ok else divergence['verdict'],
            'exclusions': proved['exclusions'],
        }
    cached.parent.mkdir(parents=True, exist_ok=True)
    cached.write_text(json.dumps(verdict))
    return verdict


def recording_bug(result):
    checked = result['faithful']
    if checked.get('ok') is False:
        return f'a recording bug: {checked["ended"]}'
    return None


def pull(connection, name=None, directory=Path('.local/traces'), tool=trace_tool, check=faithful):
    """Pull the trace `name` (the newest by default), list its marks, and check it replays faithfully."""
    started, timing = time.monotonic(), {}
    result = session(
        connection, timing, 'trace_session', lambda transfer: volumes.trace_pull(transfer, name, directory)
    )
    path = Path(result['path'])
    marks = json.loads(tool('find', path, 'mark', '--limit', 'all', '--json'))
    build = json.loads(tool('summary', path, '--json'))['header']['build']
    checked = time.monotonic()
    result['faithful'] = check(path, build) if build else {'skipped': 'the trace names no build'}
    timing['faithful_s'] = round(time.monotonic() - checked, 3)
    timing['total_s'] = round(time.monotonic() - started, 3)
    result.update(marks=marks['found'], timing=timing)
    return result
