"""Review verdicts, recorded against the patch the reviewer read."""

import argparse
import json
import subprocess
import sys
import time
from dataclasses import asdict, dataclass

from cycling_devtools import forge, workspace
from cycling_devtools.tree import git

VERDICTS = ('PASS', 'FAIL')


@dataclass(frozen=True)
class Verdict:
    patch: str
    by: str
    verdict: str
    at: float
    seconds: float | None = None


@dataclass(frozen=True)
class Step:
    """A step that was timed and decides nothing, such as /deslop."""

    patch: str
    by: str
    at: float
    seconds: float | None = None


KINDS = {'verdict': Verdict, 'step': Step}


def records():
    return workspace.main_checkout() / '.local' / 'reviews.jsonl'


def patch_id(head, base):
    """The id of what `gh pr diff` shows a reviewer: the head against its merge-base with the pull request's base."""
    fork = git('merge-base', base, head)
    diff = subprocess.run(
        ['git', 'diff', '--no-ext-diff', '--no-color', fork, head], cwd=workspace.ROOT, check=True, capture_output=True
    ).stdout
    # --stable ignores whitespace, which is code in Python and inside a string.
    out = subprocess.run(
        ['git', 'patch-id', '--verbatim'], cwd=workspace.ROOT, input=diff, check=True, capture_output=True
    )
    if not out.stdout:
        raise SystemExit(f'cycling review: {head} changes nothing against {base}')
    return out.stdout.split()[0].decode()


def record(entry):
    path = records()
    path.parent.mkdir(parents=True, exist_ok=True)
    kind = next(name for name, shape in KINDS.items() if isinstance(entry, shape))
    with path.open('a') as out:
        out.write(json.dumps({'kind': kind} | asdict(entry)) + '\n')


def latest(patch):
    try:
        lines = records().read_text().splitlines()
    except OSError:
        return [], []
    found = {kind: {} for kind in KINDS}
    for line in lines:
        fields = json.loads(line)
        kind = fields.pop('kind')
        entry = KINDS[kind](**fields)
        if entry.patch == patch:
            found[kind][entry.by] = entry
    return list(found['verdict'].values()), list(found['step'].values())


def passed(verdicts):
    words = {verdict.verdict for verdict in verdicts}
    return 'PASS' in words and 'FAIL' not in words


def table(patch, verdicts, steps):
    rows = ['| review | verdict | submit to result |', '| --- | --- | --- |']
    for entry in (*steps, *verdicts):
        took = '' if entry.seconds is None else f'{entry.seconds:.0f} s'
        rows.append(f'| {entry.by} | {getattr(entry, "verdict", "")} | {took} |')
    state = 'has a PASS' if passed(verdicts) else 'has no PASS' if verdicts else 'has no review'
    return f'Patch `{patch[:12]}` {state}.\n\n' + '\n'.join(rows)


def show(patch):
    verdicts, steps = latest(patch)
    print(table(patch, verdicts, steps))
    return verdicts


def took(args, now):
    return None if args.started is None else now - args.started


def show_patch(args, patch):
    print(patch)
    return 0


def record_verdict(args, patch):
    if args.patch != patch:
        raise SystemExit(
            f'cycling review: the review read patch {args.patch[:12]}, and the patch is now {patch[:12]}. '
            'Nothing was recorded; review the current patch.'
        )
    now = time.time()
    record(Verdict(patch, args.by, args.verdict, now, took(args, now)))
    show(patch)
    return 0


def record_step(args, patch):
    now = time.time()
    record(Step(patch, args.by, now, took(args, now)))
    show(patch)
    return 0


def show_status(args, patch):
    return 0 if passed(show(patch)) else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest='command', required=True)
    showing = commands.add_parser('patch', help='print the id of the current patch, for the reviewers to quote')
    showing.set_defaults(run=show_patch)
    recording = commands.add_parser('record', help='record one verdict on the patch that was reviewed')
    recording.add_argument('--patch', required=True, help='`cycling review patch` from when the review was submitted')
    recording.add_argument('--verdict', required=True, choices=VERDICTS)
    recording.set_defaults(run=record_verdict)
    stepping = commands.add_parser('step', help='record the time of a step that gives no verdict, such as /deslop')
    stepping.set_defaults(run=record_step)
    status = commands.add_parser('status', help='the verdicts on the current patch; fails without a PASS')
    status.set_defaults(run=show_status)
    for command in (recording, stepping):
        command.add_argument('--by', required=True, help='who did it, such as thermos-bugs/opus or deslop')
        command.add_argument('--started', type=float, help='`date +%%s` when it was submitted')
    for command in (showing, recording, stepping, status):
        command.add_argument('--pr', type=int, required=True, help='the pull request under review')
    args = parser.parse_args(argv)

    pr = forge.pull_request(args.pr)
    return args.run(args, patch_id(forge.fetch(pr).head, f'origin/{pr.base}'))


if __name__ == '__main__':
    sys.exit(main())
