"""Local replacement for CI: the gates the changed paths select, in parallel lanes, one pass/fail table."""

import argparse
import contextlib
import dataclasses
import os
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools import admission, lanes, runner, tree, workspace
from cycling_devtools.clean import warn_if_low_disk
from cycling_devtools.lanes import Gate
from cycling_devtools.runner import Result
from cycling_devtools.tree import git

TESTS_PASSED = 'tests-passed'


@dataclass(frozen=True)
class Request:
    root: Path
    merge_base: str
    pr: int | None = None
    video: bool = False
    everything: bool = False
    fresh: bool = False
    deep: bool = False

    def command(self, gate):
        if not gate.reports:
            return gate.command
        posting = (f' --pr {self.pr}' if self.pr else '') + (' --video' if self.video else '')
        return f'{gate.command} --base {self.merge_base}{posting}'


@dataclass(frozen=True)
class Plan:
    title: str
    parallel: tuple[tuple[Gate, ...], ...]
    alone: tuple[Gate, ...] = ()
    skipped: tuple[str, ...] = ()
    takes_slot: bool = True
    marks_tests: bool = True

    @property
    def gates(self):
        return [gate for lane in (*self.parallel, self.alone) for gate in lane]


def plan_for(request, watch):
    if request.deep:
        return Plan('cycling verify --deep', (lanes.DEEP,), takes_slot=False, marks_tests=False)
    if request.everything:
        names = lanes.EVERY
    else:
        names = lanes.select(tree.changed_paths(request.merge_base, watch.tree), request.root)
    *parallel, alone = (tuple(gate for gate in lane if gate.name in names) for lane in (*lanes.LANES, lanes.ALONE))
    return Plan('cycling verify', tuple(filter(None, parallel)), alone, tuple(sorted(lanes.EVERY - names)))


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
    def __init__(self, request, watch, plan, logs):
        self.request = request
        self.watch = watch
        self.plan = plan
        self.logs = logs
        self.gates = runner.Gates(request.root, logs)
        self.lock = threading.Lock()
        self.tests_out = {gate.name for gate in plan.gates if gate.test}
        self.edited = []
        self.find_cached()

    def find_cached(self):
        self.cached = {
            gate.name: seconds
            for gate in self.plan.gates
            if not self.request.fresh and (seconds := tree.cached_pass(self.lookup_key(gate))) is not None
        }

    def key(self, gate, request):
        return tree.result_key(self.watch.tree, gate.name, request.command(gate), gate.env)

    def lookup_key(self, gate):
        return self.key(gate, self.request)

    def record_keys(self, gate):
        # A pass that posted on a pull request also certifies the run that posts nothing, never the reverse.
        return {self.lookup_key(gate), self.key(gate, dataclasses.replace(self.request, pr=None, video=False))}

    def gate(self, gate):
        if gate.name in self.cached:
            return Result(gate, True, self.cached[gate.name], cached=True)
        if report := frame_report(gate, self.request.root):
            report.unlink(missing_ok=True)
        result = self.gates.run(gate, self.request.command(gate))
        result.ok = result.ok and not self.stop_if_edited()
        if result.ok:
            for key in self.record_keys(gate):
                tree.record_pass(key, gate.name, self.watch.tree, result.seconds)
        return result

    def stop_if_edited(self):
        with self.lock:
            if not self.edited:
                self.edited = self.watch.changed()
                if self.edited:
                    self.gates.stop()
            return bool(self.edited)

    def lane(self, lane):
        results = []
        for gate in lane:
            if self.edited:
                break
            result = self.gate(gate)
            if self.edited:
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


def table(title, results, skipped, watch, total, queued, cpu):
    rows = ['| step | result | time |', '| --- | --- | --- |']
    for r in results:
        took = f'cached ({r.seconds:.0f} s)' if r.cached else f'{r.seconds:.0f} s'
        rows.append(f'| `{r.gate.name}` | {"✅" if r.ok else "❌"} | {took} |')
    sha = git('rev-parse', '--short=12', 'HEAD', cwd=watch.root)
    state = '' if head_tree(watch.root) == watch.tree else ' with uncommitted changes'
    where = f'{sha}{state}, tree {watch.tree[:12]}'
    took = f'{total:.0f} s' + (f' ({queued:.0f} s queued)' if queued >= 1 else '')
    head = f'### `{title}` on {os.uname().nodename} — {where}, {took}, {cpu:.0f} CPU-s'
    notes = f'\n\nNot selected by the changed paths: {", ".join(f"`{name}`" for name in skipped)}.' if skipped else ''
    return f'{head}\n\n' + '\n'.join(rows) + notes


def head_tree(root):
    return git('rev-parse', 'HEAD^{tree}', cwd=root)


def verify(request):
    watch = tree.Watch(request.root)
    if request.pr and (loose := tree.changed_paths(head_tree(request.root), watch.tree)):
        files = '\n'.join(loose)
        raise SystemExit(
            'cycling verify --pr: results posted on a pull request must describe a commit, '
            f'and these files differ from HEAD. Commit or remove them.\n\n{files}'
        )
    plan = plan_for(request, watch)
    logs = request.root / '.local' / 'verify' / watch.tree[:12]
    logs.mkdir(parents=True, exist_ok=True)
    (logs / TESTS_PASSED).unlink(missing_ok=True)
    print(f'logs: {logs}', flush=True)

    started, cpu = time.monotonic(), runner.children_cpu()
    run = Run(request, watch, plan, logs)
    if plan.marks_tests and not run.tests_out:
        run.tests_passed()
    with contextlib.ExitStack() as held:
        queued = 0.0
        if plan.takes_slot and len(run.cached) < len(plan.gates):
            queued = held.enter_context(admission.admitted())
            run.stop_if_edited()
            run.find_cached()
        with ThreadPoolExecutor(len(plan.parallel) or 1) as pool:
            try:
                results = [r for lane in pool.map(run.lane, plan.parallel) for r in lane]
                if all(r.ok for r in results):
                    results += run.lane(plan.alone)
            except BaseException:
                # The pool waits for its lanes, and an interrupt reaches no gate's process group.
                run.gates.stop()
                raise
    total, cpu = time.monotonic() - started, runner.children_cpu() - cpu

    if run.stop_if_edited():
        files = '\n'.join(run.edited)
        return Outcome(False, f'`cycling verify` stopped: the tree changed while its gates ran.\n\n{files}', '', '')
    frames = [
        path.read_text()
        for r in results
        if not r.cached and (path := frame_report(r.gate, request.root)) and path.exists()
    ]
    return Outcome(
        all(r.ok for r in results),
        table(plan.title, results, plan.skipped, watch, total, queued, cpu),
        runner.failure_tail(results),
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
    parser.add_argument(
        '--deep',
        action='store_true',
        help='tier 2 only: the fixed fuzz seeds, the planted bug and the large Follower searches',
    )
    args = parser.parse_args(argv)
    warn_if_low_disk()
    runner.unwind_on_termination()

    request = Request(workspace.ROOT, tree.merge_base('HEAD'), args.pr, args.video, args.all, args.fresh, args.deep)
    outcome = verify(request)

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
