import subprocess
import sys
import traceback

from cycling_devtools import clean, forge, merge_checkout, workspace
from cycling_devtools.tree import git

COMMAND = 'cycling test-slow'
MIN_FREE_GB = 10


def run(number, commit):
    home = workspace.main_checkout()
    with merge_checkout.locked(merge_checkout.directory() / 'slow-tier.lock'):
        git('fetch', '--quiet', 'origin', '+refs/heads/main:refs/remotes/origin/main', cwd=home)
        if git('rev-parse', 'origin/main', cwd=home) != commit:
            print(f'slow tier: main has moved past {commit}; the newer merge runs it', flush=True)
            return 0
        if (free := clean.free_gb(home)) < MIN_FREE_GB:
            forge.comment(
                number,
                f'The slow tier did not run on {commit}, the merge of this pull request: {free:.0f} GB free, '
                f'under {MIN_FREE_GB} GB. Free disk (`cycling clean --apply`) and run `{COMMAND}` on main.',
            )
            return 1
        done = subprocess.run(
            ['nice', '-n', '19', 'bash', '-c', COMMAND],
            cwd=merge_checkout.checkout('slow-tier', commit),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
    print(f'$ {COMMAND}\n{done.stdout}', flush=True)
    if done.returncode:
        tail = '\n'.join(done.stdout.splitlines()[-30:])
        forge.comment(
            number,
            f'The slow tier failed on {commit}, the merge of this pull request.\n\n```\n$ {COMMAND}\n{tail}\n```',
        )
    return 1 if done.returncode else 0


def main(argv=None):
    number, commit = argv or sys.argv[1:]
    try:
        return run(int(number), commit)
    except Exception:
        # Nobody watches a detached run, so the pull request is where a broken one is seen.
        forge.comment(
            int(number),
            f'The slow tier could not run on {commit}, the merge of this pull request.\n\n'
            f'```\n{traceback.format_exc()}\n```',
        )
        raise


if __name__ == '__main__':
    sys.exit(main())
