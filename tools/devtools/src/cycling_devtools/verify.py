"""Local replacement for CI: every gate in parallel lanes, one pass/fail table.

Lanes that share a cargo target dir would only queue on its lock, so clippy
gets its own dir and each firmware variant already has one; sccache shares the
dependency builds between them. harness-virtual has real-time reply timeouts,
so it runs alone after the loaded phase.
"""

import argparse
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools.clean import warn_if_low_disk
from cycling_devtools.workspace import ROOT


@dataclass(frozen=True)
class Step:
    name: str
    command: str
    env: tuple[tuple[str, str], ...] = ()


def build(sdk, harness):
    """One variant; its image stays apart from .local/cycling.bin and the other lanes."""
    name = f'build-sdk{sdk}-h{harness}'
    env = (
        ('CYCLING_SDK', sdk),
        ('CYCLING_HARNESS', harness),
        ('CYCLING_IMAGE_DIR', f'.local/verify/images/{name}'),
    )
    return Step(name, 'mise run build', env)


# Each inner list runs in order; the lists run in parallel.
LANES = {
    'full': [
        [Step('test', 'mise run test'), Step('simulate', 'mise run simulate')],
        [Step('check', 'mise run check', (('CARGO_TARGET_DIR', 'target/clippy'),))],
        *[[build(sdk, harness)] for sdk in '01' for harness in '01'],
    ],
    # The inner loop before pushing: everything except three of the four builds.
    'fast': [
        [Step('test', 'mise run test')],
        [Step('check', 'mise run check', (('CARGO_TARGET_DIR', 'target/clippy'),))],
        [build('1', '1')],
    ],
}
ALONE = {'full': [Step('harness-virtual', 'mise run harness-virtual')], 'fast': []}


@dataclass
class Result:
    step: Step
    ok: bool
    seconds: float
    log: Path


def run_step(step, logs):
    log = logs / f'{step.name}.log'
    env = os.environ | dict(step.env)
    started = time.monotonic()
    with log.open('w') as out:
        out.write(f'$ {step.command}\n')
        out.flush()
        code = subprocess.run(
            step.command, shell=True, cwd=ROOT, env=env, stdout=out, stderr=subprocess.STDOUT
        ).returncode
    return Result(step, code == 0, time.monotonic() - started, log)


def run_lane(lane, logs):
    results = []
    for step in lane:
        result = run_step(step, logs)
        results.append(result)
        print(f'{"ok  " if result.ok else "FAIL"} {step.name} ({result.seconds:.0f} s)', flush=True)
        if not result.ok:
            break
    return results


def git(*args):
    return subprocess.run(['git', *args], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()


def table(results, sha, dirty, total):
    rows = ['| step | result | time |', '| --- | --- | --- |']
    rows += [f'| `{r.step.name}` | {"✅" if r.ok else "❌"} | {r.seconds:.0f} s |' for r in results]
    state = ' (uncommitted changes)' if dirty else ''
    return f'### `mise run verify` on {os.uname().nodename} — {sha}{state}, {total:.0f} s\n\n' + '\n'.join(rows)


def failure_tail(results, lines=30):
    parts = []
    for result in results:
        if not result.ok:
            tail = result.log.read_text(errors='replace').splitlines()[-lines:]
            parts.append(f'--- {result.step.name}\n' + '\n'.join(tail))
    return '\n'.join(parts)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--fast', action='store_true', help='test, check and the SDK=1 HARNESS=1 build only')
    parser.add_argument('--pr', type=int, help='post the result table on this pull request')
    args = parser.parse_args(argv)
    warn_if_low_disk()
    mode = 'fast' if args.fast else 'full'

    sha = git('rev-parse', '--short=12', 'HEAD')
    dirty = bool(git('status', '--porcelain', '--untracked-files=no'))
    if args.pr and dirty:
        parser.error('commit first: results posted on a PR must describe a commit')
    logs = ROOT / '.local' / 'verify' / sha
    logs.mkdir(parents=True, exist_ok=True)

    started = time.monotonic()
    with ThreadPoolExecutor(len(LANES[mode])) as pool:
        results = [r for lane in pool.map(lambda lane: run_lane(lane, logs), LANES[mode]) for r in lane]
    if all(r.ok for r in results):
        results += run_lane(ALONE[mode], logs)
    total = time.monotonic() - started
    ok = all(r.ok for r in results)

    report = table(results, sha, dirty, total)
    tail = failure_tail(results)
    print('\n' + report)
    if tail:
        print('\n' + tail, file=sys.stderr)
    print(f'\nlogs: {logs.relative_to(ROOT)}')
    if args.pr:
        body = report + (f'\n\n<details><summary>failures</summary>\n\n```\n{tail}\n```\n</details>' if tail else '')
        subprocess.run(['gh', 'pr', 'comment', str(args.pr), '--body', body], cwd=ROOT, check=True)
        print(f'posted to #{args.pr}')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
