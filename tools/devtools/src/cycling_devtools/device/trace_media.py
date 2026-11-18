"""Pull a ride trace, then judge it on this machine with the build that recorded it: list the moments the rider
marked in it, and check once that it replays faithfully, where a divergence is a recording bug."""

import hashlib
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

from cycling_devtools import twin, upgrade, workspace
from cycling_devtools.device import volumes
from cycling_devtools.workspace import ROOT

NO_BUILD = '00' * 20


class Skipped(Exception):
    """No twin of the recording build can judge the trace here."""


def recorded_twin(twin_for, build, root):
    try:
        return twin_for(build, root=root)
    except (twin.NotArchived, twin.Incapable) as error:
        raise Skipped(str(error)) from None
    except (OSError, subprocess.CalledProcessError, RuntimeError) as error:
        raise Skipped(f'the twin of build {build[:12]} did not build: {error}') from None


def recorded_marks(trace, build, *, root=ROOT, twin_for=twin.binary_for_build):
    """The marks in `trace`, listed by the trace tool of `build`, the build that recorded it, or {skipped: why}."""
    try:
        binary = recorded_twin(twin_for, build, root)
        listed = twin.run_json(binary, 'find', str(trace), 'mark', '--limit', 'all')
    except Skipped as why:
        return {'skipped': str(why)}
    except OSError as error:
        return {'skipped': did_not_start(build, error)}
    if 'found' not in listed:
        return {'skipped': f'the trace tool of build {build[:12]} lists no marks: {listed.get("error")}'}
    return listed['found']


def did_not_start(build, error):
    return f'the twin of build {build[:12]} did not start: {error}'


def marks_text(marks):
    if isinstance(marks, dict):
        return f'marks: unread, {marks["skipped"]}'
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


def replay_faithful(binary, trace, root=ROOT):
    """The faithful replay's JSON document, or {error}."""
    env = os.environ | {'CYCLING_TERRAIN_PACKS': str(workspace.terrain_packs(root))}
    with tempfile.TemporaryDirectory(prefix='faithful-') as out:
        command = [str(binary), 'replay', str(trace), '--mode', 'faithful', '--to', '99999:00']
        command += ['--every', '99999:00', '--out', str(Path(out) / 'replay'), '--json']
        done = subprocess.run(command, capture_output=True, text=True, env=env)
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
        binary = recorded_twin(twin_for, build, root)
        if why := check(binary, build, root=root):
            return {'skipped': why}
        replayed = replay(binary, trace, root=root)
    except Skipped as why:
        return {'skipped': str(why)}
    except (OSError, subprocess.CalledProcessError) as error:
        return {'skipped': did_not_start(build, error)}
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
    checked = result.get('faithful', {})
    if checked.get('ok') is False:
        return f'a recording bug: {checked["ended"]}'
    return None


def pull(connection, name=None, directory=Path('.local/traces')):
    """Copy the trace `name`, the newest by default, and nothing else."""
    timing = {}
    result = session(
        connection, timing, 'trace_session', lambda transfer: volumes.trace_pull(transfer, name, directory)
    )
    sha256 = hashlib.sha256(Path(result['path']).read_bytes()).hexdigest()
    return {**result, 'sha256': sha256, 'timing': timing}


def judge(result, *, marks_of=recorded_marks, check=faithful):
    """Add to a pulled `result` its trace's marks and faithful verdict, each from the build that recorded it."""
    started = time.monotonic()
    path = Path(result['path'])
    try:
        _, build = upgrade.header(path)
        why = 'the trace names no build' if build == NO_BUILD else None
    except ValueError as error:
        why = str(error)
    if why:
        result['marks'] = result['faithful'] = {'skipped': why}
    else:
        result['marks'] = marks_of(path, build)
        result['faithful'] = check(path, build)
    result['timing']['judge_s'] = round(time.monotonic() - started, 3)
    return result
