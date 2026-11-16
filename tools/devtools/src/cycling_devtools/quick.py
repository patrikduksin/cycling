"""Tier 0, the inner loop: the crates the diff touches, checked in place within seconds.

It runs in the worktree itself and records nothing: `cycling verify` is what
certifies a tree. Tests leave out nextest's `slow` group.
"""

import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass

from cycling_devtools import lanes, tree, verify, workspace


@dataclass(frozen=True)
class Step:
    name: str
    command: str
    env: tuple[tuple[str, str], ...] = ()


def touched(paths, root):
    """Directory and package name of each workspace crate the paths touch."""
    return {
        directory: name
        for directory, (name, _) in sorted(lanes.members(root).items())
        if any(path.startswith(f'{directory}/') for path in paths)
    }


def steps(crates, firmware):
    """What to run for the touched crates; the firmware check only when a device links one of them."""
    if not crates:
        return []
    packages = ' '.join(f'-p {name}' for name in crates.values())
    found = [
        Step(
            'test', f"cargo nextest run --locked --no-default-features --no-tests=pass {packages} -E 'not group(slow)'"
        ),
        # Its own target dir, as in verify, so it does not queue on the tests' build lock.
        Step(
            'clippy',
            f'cargo clippy --locked --no-default-features --all-targets {packages} -- -D warnings',
            (('CARGO_TARGET_DIR', 'target/clippy'),),
        ),
    ]
    if crates.keys() & firmware:
        found.append(Step('check-firmware', 'cycling check-firmware'))
    return found


def run(step, logs):
    log = logs / f'{step.name}.log'
    started = time.monotonic()
    with log.open('w') as out:
        code = subprocess.run(
            step.command,
            shell=True,
            cwd=workspace.ROOT,
            env=os.environ | dict(step.env),
            stdout=out,
            stderr=subprocess.STDOUT,
        ).returncode
    seconds = time.monotonic() - started
    print(f'{"ok  " if code == 0 else "FAIL"} {step.name} ({seconds:.1f} s)', flush=True)
    return step, code == 0, log


def main():
    root = workspace.ROOT
    paths = verify.changed_paths(verify.merge_base('HEAD'), tree.tree_hash(root))
    crates = touched(paths, root)
    plan = steps(crates, lanes.firmware_crates(root))
    if not plan:
        print('cycling quick: the diff touches no Rust crate')
        return 0
    print(f'cycling quick: {", ".join(crates.values())}', flush=True)
    logs = root / '.local' / 'verify' / 'quick'
    logs.mkdir(parents=True, exist_ok=True)
    started, cpu = time.monotonic(), verify.children_cpu()
    with ThreadPoolExecutor(len(plan)) as pool:
        results = list(pool.map(lambda step: run(step, logs), plan))
    print(f'{time.monotonic() - started:.1f} s, {verify.children_cpu() - cpu:.0f} CPU-s')
    for step, ok, log in results:
        if not ok:
            tail = log.read_text(errors='replace').splitlines()[-30:]
            print(f'--- {step.name}\n' + '\n'.join(tail), file=sys.stderr)
    return 0 if all(ok for _, ok, _ in results) else 1


if __name__ == '__main__':
    sys.exit(main())
