"""Local replacement for CI: the gates the changed paths select, in parallel lanes, one pass/fail table.

Verify certifies a tree, not a worktree. The gates run in the worktree, and a
run whose files change under it stops and certifies nothing (tree.py). Each
gate that passes is recorded under the tree's hash, the toolchain and its own
definition, so the same tree is never verified twice, here or in another
worktree.
"""

import argparse
import contextlib
import os
import signal
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools import admission, lanes, tree, workspace
from cycling_devtools.clean import warn_if_low_disk
from cycling_devtools.tree import git

# Printed, and written as a file of this name in the log directory, once every selected test gate has passed.
TESTS_PASSED = 'tests-passed'


@dataclass(frozen=True)
class Request:
    root: Path
    # The commit the changed paths are counted against and the screens are compared with.
    base: str
    pr: int | None = None
    video: bool = False
    everything: bool = False
    fresh: bool = False
    # Tier 2 alone, whatever paths changed.
    deep: bool = False

    def command(self, gate):
        if not gate.reports:
            return gate.command
        return gate.command + (f' --pr {self.pr}' if self.pr else '') + (' --video' if self.video else '')


@dataclass
class Result:
    gate: lanes.Gate
    ok: bool
    seconds: float
    log: Path | None = None
    cached: bool = False


@dataclass
class Outcome:
    ok: bool
    report: str
    failures: str
    frames: str

    def comment(self):
        body = self.report
        if self.failures:
            body += f'\n\n<details><summary>failures</summary>\n\n```\n{self.failures}\n```\n</details>'
        if self.frames:
            body += f'\n\n<details><summary>largest stack frames</summary>\n\n{self.frames}\n</details>'
        return body


class Run:
    """One verify of one tree: what its gates are keyed by, which tests are still out, and what changed under it."""

    def __init__(self, request, watch, selected, logs):
        self.request = request
        self.watch = watch
        self.logs = logs
        self.lock = threading.Lock()
        self.selected = selected
        self.tests_out = {gate.name for gate in selected if gate.test}
        self.running = set()
        self.changed = []
        self.find_cached()

    def find_cached(self):
        self.cached = {
            gate.name: seconds
            for gate in self.selected
            if not self.request.fresh and (seconds := tree.cached_pass(self.keys(gate)[0])) is not None
        }

    def keys(self, gate):
        """The keys a pass of gate is recorded under; a cached pass is looked up under the first.

        A run that posts on a pull request must happen for that pull request, and
        its pass also stands for a run that posts nothing, such as the merge's.
        """
        base = self.request.base if gate.reports else None
        commands = dict.fromkeys((self.request.command(gate), gate.command))
        return [tree.result_key(self.watch.tree, gate.name, command, gate.env, base) for command in commands]

    def gate(self, gate):
        if gate.name in self.cached:
            return Result(gate, True, self.cached[gate.name], cached=True)
        log = self.logs / f'{gate.name}.log'
        command = self.request.command(gate)
        if report := frame_report(gate, self.request.root):
            report.unlink(missing_ok=True)
        started = time.monotonic()
        with log.open('w') as out:
            out.write(f'$ {command}\n')
            out.flush()
            # Started under the lock: stop() either finds this gate running or has already kept it from starting.
            with self.lock:
                if self.changed:
                    return Result(gate, False, 0.0, log)
                # Its own process group, so stop() reaches the compilers and tests below the shell.
                process = subprocess.Popen(
                    command,
                    shell=True,
                    cwd=self.request.root,
                    env=os.environ | dict(gate.env),
                    stdout=out,
                    stderr=subprocess.STDOUT,
                    process_group=0,
                )
                self.running.add(process)
            code = process.wait()
            with self.lock:
                self.running.discard(process)
        seconds = time.monotonic() - started
        ok = code == 0 and not self.tree_changed()
        if ok:
            for key in self.keys(gate):
                tree.record_pass(key, gate.name, self.watch.tree, seconds)
        return Result(gate, ok, seconds, log)

    def tree_changed(self):
        """Whether the tree differs from the one this run certifies. The first gate to see it stops the others."""
        with self.lock:
            if not self.changed:
                self.changed = self.watch.changed()
                if self.changed:
                    self.stop()
            return bool(self.changed)

    def stop(self):
        for process in self.running:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGTERM)

    def lane(self, lane):
        results = []
        for gate in lane:
            if self.changed:
                break
            result = self.gate(gate)
            if self.changed:
                break
            results.append(result)
            took = 'cached' if result.cached else f'{result.seconds:.0f} s'
            print(f'{"ok  " if result.ok else "FAIL"} {gate.name} ({took})', flush=True)
            if not result.ok:
                break
            self.passed(gate)
        return results

    def passed(self, gate):
        with self.lock:
            if gate.name in self.tests_out:
                self.tests_out.discard(gate.name)
                if not self.tests_out:
                    self.tests_passed()

    def tests_passed(self):
        (self.logs / TESTS_PASSED).touch()
        print(f'verify: tests passed ({self.logs / TESTS_PASSED})', flush=True)


def frame_report(gate, root):
    image_dir = dict(gate.env).get('CYCLING_IMAGE_DIR')
    return root / image_dir / 'stack-frames.md' if image_dir else None


def merge_base(commit):
    """Where commit left main: the tree its changed paths are counted against."""
    for main in ('origin/main', 'main'):
        done = subprocess.run(['git', 'merge-base', main, commit], cwd=workspace.ROOT, capture_output=True, text=True)
        if done.returncode == 0:
            return done.stdout.strip()
    raise SystemExit('cycling verify: no origin/main or main to compare with')


def changed_paths(base, tree_id):
    return git('diff', '--name-only', '--no-renames', base, tree_id).splitlines()


def table(command, results, skipped, watch, total, queued, cpu):
    rows = ['| step | result | time |', '| --- | --- | --- |']
    for r in results:
        took = f'cached ({r.seconds:.0f} s)' if r.cached else f'{r.seconds:.0f} s'
        rows.append(f'| `{r.gate.name}` | {"✅" if r.ok else "❌"} | {took} |')
    sha = git('rev-parse', '--short=12', 'HEAD', cwd=watch.root)
    state = '' if git('rev-parse', 'HEAD^{tree}', cwd=watch.root) == watch.tree else ' with uncommitted changes'
    where = f'{sha}{state}, tree {watch.tree[:12]}'
    took = f'{total:.0f} s' + (f' ({queued:.0f} s queued)' if queued >= 1 else '')
    head = f'### `{command}` on {os.uname().nodename} — {where}, {took}, {cpu:.0f} CPU-s'
    notes = f'\n\nNot selected by the changed paths: {", ".join(f"`{name}`" for name in skipped)}.' if skipped else ''
    return f'{head}\n\n' + '\n'.join(rows) + notes


def failure_tail(results, lines=30):
    parts = []
    for result in results:
        if not result.ok:
            tail = result.log.read_text(errors='replace').splitlines()[-lines:]
            parts.append(f'--- {result.gate.name}\n' + '\n'.join(tail))
    return '\n'.join(parts)


def children_cpu():
    times = os.times()
    return times.children_user + times.children_system


def verify(request):
    """Run the gates that the tree of request.root needs and return the Outcome.

    A run with gates to run waits for one of the machine's slots first, and its time counts the wait.
    """
    watch = tree.Watch(request.root)
    if request.deep:
        chosen, skipped = [list(lanes.DEEP), []], []
    else:
        names = lanes.EVERY if request.everything else lanes.select(changed_paths(request.base, watch.tree))
        chosen = [[gate for gate in lane if gate.name in names] for lane in (*lanes.LANES, lanes.ALONE)]
        skipped = sorted(lanes.EVERY - names)
    *parallel, alone = chosen
    parallel = [lane for lane in parallel if lane]
    logs = request.root / '.local' / 'verify' / watch.tree[:12]
    logs.mkdir(parents=True, exist_ok=True)
    (logs / TESTS_PASSED).unlink(missing_ok=True)
    print(f'logs: {logs}', flush=True)

    started, cpu = time.monotonic(), children_cpu()
    selected = [gate for lane in chosen for gate in lane]
    run = Run(request, watch, selected, logs)
    if not run.tests_out and not request.deep:
        run.tests_passed()
    with contextlib.ExitStack() as held:
        # Tier 2 runs at nice 19 and holds no slot against the pull requests' runs.
        waits = not request.deep and len(run.cached) < len(selected)
        queued = held.enter_context(admission.admitted()) if waits else 0.0
        if queued >= 1:
            run.tree_changed()
            # Another run may have passed this tree while this one waited.
            run.find_cached()
        with ThreadPoolExecutor(len(parallel) or 1) as pool:
            try:
                results = [r for lane in pool.map(run.lane, parallel) for r in lane]
            except BaseException:
                # The pool waits for its lanes, and an interrupt reaches no gate's process group.
                with run.lock:
                    run.stop()
                raise
        if all(r.ok for r in results):
            results += run.lane(alone)
    total, cpu = time.monotonic() - started, children_cpu() - cpu

    if run.changed:
        files = '\n'.join(run.changed)
        return Outcome(False, f'`cycling verify` stopped: the tree changed while its gates ran.\n\n{files}', '', '')
    frames = [
        path.read_text()
        for r in results
        if not r.cached and (path := frame_report(r.gate, request.root)) and path.exists()
    ]
    return Outcome(
        all(r.ok for r in results),
        table(
            'cycling verify --deep' if request.deep else 'cycling verify', results, skipped, watch, total, queued, cpu
        ),
        failure_tail(results),
        '\n'.join(frames),
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--pr', type=int, help='post the result table and the changed screens on this pull request')
    parser.add_argument(
        '--video',
        action='store_true',
        help='with the screens, before and after videos of scenarios whose screens changed',
    )
    parser.add_argument('--all', action='store_true', help='every gate, whatever paths changed')
    parser.add_argument('--fresh', action='store_true', help='run every selected gate again, ignoring cached passes')
    parser.add_argument('--deep', action='store_true', help='tier 2 only: the fixed fuzz seeds and the planted bug')
    args = parser.parse_args(argv)
    warn_if_low_disk()

    if args.pr and tree.tree_hash(workspace.ROOT) != git('rev-parse', 'HEAD^{tree}'):
        parser.error('commit first: results posted on a PR must describe a commit')
    outcome = verify(Request(workspace.ROOT, merge_base('HEAD'), args.pr, args.video, args.all, args.fresh, args.deep))

    print('\n' + outcome.report)
    if outcome.failures:
        print('\n' + outcome.failures, file=sys.stderr)
    if args.pr:
        subprocess.run(
            ['gh', 'pr', 'comment', str(args.pr), '--body', outcome.comment()], cwd=workspace.ROOT, check=True
        )
        print(f'posted to #{args.pr}')
    return 0 if outcome.ok else 1


if __name__ == '__main__':
    sys.exit(main())
