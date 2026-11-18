"""Tier 2 on the newest main: `cycling verify --deep` after each merge and every night, one run at a time.

A request leaves the marker .local/nightly/deep.pending in the primary checkout and takes the lock beside it
when the lock is free. The holder clears the marker, fetches main, and runs the deep verify on it in the locked
worktree .local/merge/deep. While a marker is back when it finishes, it runs again on the newest main, so the
requests made during a run become one more run. A request that finds the lock held returns at once.

.local/nightly/deep.json holds the last green commit and the last run. A failure bisects the commits since the
last green one with the gate that failed alone, and files one GitHub issue naming the first bad commit. A gate
that passes when run again on the same commit files nothing. A deep verify that stopped because the tree changed
under it, as when a ride lands in the corpus, runs again.
"""

import fcntl
import json
import re
import subprocess
import sys
from dataclasses import dataclass

from cycling_devtools import forge, lanes, merge_checkout, workspace
from cycling_devtools.tree import git

COMMAND = 'cycling verify --deep'
FAILED = re.compile(r'^FAIL (\S+) \(', re.MULTILINE)
STOPPED = 'stopped: the tree changed'
TAIL = 30


@dataclass(frozen=True)
class Green:
    pass


@dataclass(frozen=True)
class GateFailed:
    gate: str

    @property
    def label(self):
        return self.gate

    def bisect_command(self):
        return {gate.name: gate.command for gate in lanes.DEEP}.get(self.gate, COMMAND)


@dataclass(frozen=True)
class FailedOutsideEveryGate:
    label = 'verify'

    def bisect_command(self):
        return COMMAND


@dataclass(frozen=True)
class Stopped:
    pass


@dataclass(frozen=True)
class Run:
    commit: str
    outcome: Green | Stopped | GateFailed | FailedOutsideEveryGate = Green()
    first_bad: str | None = None

    @property
    def failed(self):
        return not isinstance(self.outcome, Green | Stopped)

    def json(self):
        label = self.outcome.label if self.failed else None
        return {'commit': self.commit, 'failed': label, 'first_bad': self.first_bad}


@dataclass(frozen=True)
class Blame:
    first_bad: str | None
    finding: str


def directory():
    return workspace.main_checkout() / '.local' / 'nightly'


def request(issues=None):
    issues = issues or forge.Issues()
    place = directory()
    place.mkdir(parents=True, exist_ok=True)
    pending = place / 'deep.pending'
    pending.touch()
    runs = []
    while pending.exists() and (lock := lock_if_free(place)):
        with lock:
            claim_pending(pending)
            try:
                run = run_newest(place / 'deep.json', issues)
            except BaseException:
                pending.touch()
                raise
            if isinstance(run.outcome, Stopped):
                pending.touch()
            runs.append(run)
    return runs


def lock_if_free(place):
    lock = (place / 'deep.lock').open('a')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lock.close()
        return None
    return lock


def claim_pending(pending):
    pending.unlink()


def run_newest(state_path, issues):
    home = workspace.main_checkout()
    git('fetch', '--quiet', 'origin', '+refs/heads/main:refs/remotes/origin/main', cwd=home)
    head = git('rev-parse', 'origin/main', cwd=home)
    state = json.loads(state_path.read_text()) if state_path.exists() else {}
    green = state.get('green')
    place = merge_checkout.checkout('deep', head)
    print(f'deep tier: {COMMAND} on {head}', flush=True)
    outcome, output = verify_deep(place)
    if isinstance(outcome, Stopped):
        print(f'deep tier: {COMMAND} stopped on {head}; it runs again', flush=True)
        return Run(head, outcome)
    if isinstance(outcome, Green):
        run, green = Run(head), head
    else:
        blamed = blame(place, green, head, outcome)
        run = Run(head, outcome, blamed and blamed.first_bad)
        if blamed:
            report(issues, run, green, blamed.finding, output)
    written = state_path.with_suffix('.tmp')
    written.write_text(json.dumps({'green': green, 'last': run.json()}))
    written.replace(state_path)
    print(f'deep tier: {run}', flush=True)
    return run


def verify_deep(place):
    lines = []
    with subprocess.Popen(
        ['bash', '-c', COMMAND], cwd=place, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True
    ) as job:
        for line in job.stdout:
            print(line, end='', flush=True)
            lines.append(line)
    output = ''.join(lines)
    if job.returncode == 0:
        return Green(), output
    if STOPPED in output:
        return Stopped(), output
    named = FAILED.search(output)
    return (GateFailed(named[1]) if named else FailedOutsideEveryGate()), output


def blame(place, green, head, failure):
    command = failure.bisect_command()
    if subprocess.run(['bash', '-c', command], cwd=place).returncode == 0:
        print(f'deep tier: `{command}` passes when run again on {head}, so nothing is filed', flush=True)
        return None
    if green is None:
        return Blame(None, 'No commit has passed tier 2 yet, so there is no range to bisect.')
    merge_checkout.checkout('deep', green)
    if subprocess.run(['bash', '-c', command], cwd=place).returncode:
        return Blame(
            None,
            f'`{command}` fails on {green}, the last green commit, too, so no commit since broke it: '
            'a ride the corpus gained since, or a gate that does not always fail.',
        )
    if green == head:
        return Blame(None, f'`{command}` fails on {head}, the last green commit, only some of the time.')
    git('bisect', 'start', head, green, cwd=place)
    try:
        # `git bisect run` skips a commit on status 125 and stops on 128 or more, so every failure exits 1.
        if subprocess.run(['git', 'bisect', 'run', 'bash', '-c', f'({command}) || exit 1'], cwd=place).returncode:
            return Blame(None, f'`git bisect run` across {green}..{head} did not finish; its output is in the log.')
        first_bad = git('rev-parse', 'refs/bisect/bad', cwd=place)
    finally:
        git('bisect', 'reset', cwd=place)
    subject = git('log', '-1', '--format=%s', first_bad, cwd=place)
    return Blame(
        first_bad,
        f'`git bisect run` across {green}..{head} with `{command}` names {first_bad} ({subject}) '
        'as the first bad commit.',
    )


def report(issues, run, green, finding, output):
    culprit = run.first_bad or f'after {green}'
    label = run.outcome.label
    title = (
        f'Tier 2: {label} fails from {run.first_bad[:12]}'
        if run.first_bad
        else f'Tier 2: {label} fails on main at {run.commit[:12]}'
    )
    tail = '\n'.join(output.splitlines()[-TAIL:])
    body = f'`{COMMAND}` failed on main at {run.commit}.\n\n{finding}\n\n```\n{tail}\n```\n'
    issues.file_once(f'<!-- cycling-deep {label} {culprit} -->', title, body)


def main():
    return 1 if any(run.failed for run in request()) else 0


if __name__ == '__main__':
    sys.exit(main())
