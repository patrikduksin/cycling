"""cycling merge <pr>: squash the pull request onto main, verify that tree, and push exactly it."""

import argparse
import subprocess
import sys

from cycling_devtools import forge, merge_checkout, review, runner, verify, workspace
from cycling_devtools.tree import git

ATTEMPTS = 5
SLOW_TIER = (sys.executable, '-m', 'cycling_devtools.slow_tier')
DEEP_TIER = (sys.executable, '-m', 'cycling_devtools.deep_tier')
FILL_SCREENS = ('nice', '-n', '19', 'cycling', 'sim-compare', '--fill')


def seconds(raw_date):
    return int(raw_date.split()[0])


def squash(heads, pr):
    merged = subprocess.run(
        ['git', 'merge-tree', '--write-tree', heads.main, heads.head],
        cwd=workspace.ROOT,
        capture_output=True,
        text=True,
    )
    if merged.returncode:
        raise SystemExit(f'cycling merge: #{pr.number} conflicts with main; rebase it and have the new patch reviewed')
    name, email, authored = git('log', '-1', '--date=raw', '--format=%an%n%ae%n%ad', heads.head).splitlines()
    # Fixed by the two heads, so a retry does not mint a new commit, and never earlier than its parent.
    committed = max(authored, git('log', '-1', '--date=raw', '--format=%cd', heads.main), key=seconds)
    dates = {
        'GIT_AUTHOR_NAME': name,
        'GIT_AUTHOR_EMAIL': email,
        'GIT_AUTHOR_DATE': authored,
        'GIT_COMMITTER_DATE': committed,
    }
    message = f'{pr.title} (#{pr.number})\n\n{pr.body}'.strip() + '\n'
    return git('commit-tree', merged.stdout.split()[0], '-p', heads.main, '-m', message, env=dates)


def push(*refspecs):
    return subprocess.run(
        ['git', 'push', '--quiet', 'origin', *refspecs], cwd=workspace.ROOT, capture_output=True, text=True
    )


def remote_main():
    done = subprocess.run(
        ['git', 'ls-remote', 'origin', 'refs/heads/main'], cwd=workspace.ROOT, capture_output=True, text=True
    )
    return done.stdout.split()[0] if done.returncode == 0 and done.stdout else None


def land(pr, heads, commit):
    branch = f'refs/heads/{pr.branch}'
    if (held := push(f'--force-with-lease={branch}:{heads.head}', f'{commit}:{branch}')).returncode:
        return held.stderr
    landed = False
    try:
        # main requires the status, and GitHub takes a status only for a commit it holds.
        forge.post_status(commit, 'success', 'cycling verify passed on this tree')
        # GitHub marks a pull request merged when main comes to hold the head it has recorded for it,
        # so main moves only once GitHub shows this head.
        if not forge.shows(pr.number, lambda shown: shown.head == commit):
            return f'GitHub did not show {commit} as the head of #{pr.number}.'
        # The branch rides along at its own value, so a push to it that origin advertises refuses both. One that
        # lands in the moment after can't be stopped, but then only the reviewed squash reaches main and the
        # pull request stays open on the newer head.
        moved = push(
            '--atomic',
            f'--force-with-lease=refs/heads/main:{heads.main}',
            f'--force-with-lease={branch}:{commit}',
            f'{commit}:refs/heads/main',
            f'{commit}:{branch}',
        )
        # A push origin applied can still be reported as failed when the connection drops.
        landed = not moved.returncode or remote_main() == commit
        return None if landed else moved.stderr
    finally:
        if not landed and remote_main() == commit:
            landed = True
        if not landed:
            back = push(f'--force-with-lease={branch}:{commit}', f'{heads.head}:{branch}')
            state = f'is back at {heads.head}.' if not back.returncode else f'was not put back: {back.stderr.strip()}'
            print(f'cycling merge: #{pr.number} did not land. Its branch {pr.branch} {state}', flush=True)


def detached(command, log, cwd):
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open('a') as out:
        return subprocess.Popen(command, cwd=cwd, stdout=out, stderr=subprocess.STDOUT, start_new_session=True)


def announce(pr, commit, report):
    git('fetch', '--quiet', '--prune', 'origin', '+refs/heads/main:refs/remotes/origin/main')
    if forge.shows(pr.number, lambda shown: shown.state == 'MERGED'):
        push(f'--force-with-lease=refs/heads/{pr.branch}:{commit}', '--delete', pr.branch)
        kept = ''
    else:
        kept = f'\n\nGitHub does not show #{pr.number} as merged, so its branch `{pr.branch}` is kept.'
        print(f'cycling merge: {kept.strip()}', flush=True)
    forge.comment(pr.number, f'Merged as {commit} by `cycling merge`.{kept}\n\n{report}')


def after(pr, commit, report):
    try:
        announce(pr, commit, report)
    except Exception as failed:
        why = getattr(failed, 'stderr', None) or failed
        print(f'cycling merge: #{pr.number} is merged, but its pull request was not updated: {why}', flush=True)
    place = merge_checkout.directory()
    detached([*FILL_SCREENS, commit], place / 'fill.log', place / 'verify')
    detached([*SLOW_TIER, str(pr.number), commit], place / 'slow-tier.log', workspace.main_checkout())
    detached(DEEP_TIER, place / 'deep.log', workspace.main_checkout())


def open_pull_request(number):
    pr = forge.pull_request(number)
    if pr.state == 'MERGED':
        print(f'#{number} is merged already')
        return None
    if pr.state != 'OPEN' or pr.draft:
        raise SystemExit(f'cycling merge: #{number} is not an open, ready pull request')
    if pr.base != 'main':
        raise SystemExit(f'cycling merge: #{number} targets {pr.base}, not main; merge what it is stacked on first')
    return pr


def squash_and_verify(pr, heads):
    commit = squash(heads, pr)
    outcome = verify.verify(verify.Request(merge_checkout.checkout('verify', commit), heads.main))
    print('\n' + outcome.report)
    if not outcome.ok:
        raise SystemExit(f'\n{outcome.failures}\n\ncycling merge: the squash of #{pr.number} onto main fails verify')
    return commit, outcome


def prepare(number):
    if pr := open_pull_request(number):
        squash_and_verify(pr, forge.fetch(pr))


def require_pass(number, patch):
    verdicts, steps = review.latest(patch)
    table = review.table(patch, verdicts, steps)
    if not review.passed(verdicts):
        raise SystemExit(f'cycling merge: #{number} needs a review PASS on its current patch.\n\n{table}')
    return table


def merge(number):
    pr = open_pull_request(number)
    if pr is None:
        return
    heads = forge.fetch(pr)
    reviewed = heads.head
    patch = review.patch_id(reviewed, heads.main)
    require_pass(number, patch)
    for _ in range(ATTEMPTS):
        commit, outcome = squash_and_verify(pr, heads)
        table = require_pass(number, patch)
        refused = land(pr, heads, commit)
        if refused is None:
            print(f'merged #{number} as {commit}', flush=True)
            after(pr, commit, f'{outcome.comment()}\n\n{table}')
            return
        now = forge.fetch(pr)
        if now.head != reviewed:
            raise SystemExit(f'cycling merge: #{number} changed during the merge; its new patch needs a review')
        if now == heads:
            raise SystemExit(
                f'cycling merge: #{number} did not land, and neither main nor its branch moved.\n\n{refused}'
            )
        print('cycling merge: main moved; starting over', flush=True)
        heads = now
    raise SystemExit(f'cycling merge: main kept moving for {ATTEMPTS} attempts')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('pr', type=int)
    parser.add_argument(
        '--prepare',
        action='store_true',
        help='squash and verify only, so the merge finds the result cached',
    )
    args = parser.parse_args(argv)
    runner.unwind_on_termination()
    print('cycling merge: waiting for the merge queue', flush=True)
    with merge_checkout.locked(merge_checkout.directory() / 'merge.lock'):
        (prepare if args.prepare else merge)(args.pr)
    return 0


if __name__ == '__main__':
    sys.exit(main())
