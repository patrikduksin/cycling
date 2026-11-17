import json
import subprocess
import time
from dataclasses import dataclass
from typing import NamedTuple

from cycling_devtools import workspace
from cycling_devtools.tree import git

STATUS_CONTEXT = 'cycling/verify'
POLLS = 30
POLL_SECONDS = 2.0


@dataclass(frozen=True)
class PullRequest:
    number: int
    branch: str
    base: str
    title: str
    body: str
    state: str
    draft: bool
    head: str


class Heads(NamedTuple):
    main: str
    head: str


def gh(*args):
    # The primary checkout outlives the worktree that started a background job.
    return subprocess.run(
        ['gh', *args], cwd=workspace.main_checkout(), check=True, capture_output=True, text=True
    ).stdout


def pull_request(number):
    fields = json.loads(
        gh('pr', 'view', str(number), '--json', 'headRefName,baseRefName,title,body,state,isDraft,headRefOid')
    )
    return PullRequest(
        number,
        fields['headRefName'],
        fields['baseRefName'],
        fields['title'],
        fields['body'],
        fields['state'],
        fields['isDraft'],
        fields['headRefOid'],
    )


def fetch(pr):
    refs = [f'+refs/heads/{name}:refs/remotes/origin/{name}' for name in {'main', pr.base, pr.branch}]
    git('fetch', '--quiet', 'origin', *refs)
    return Heads(git('rev-parse', 'origin/main'), git('rev-parse', f'origin/{pr.branch}'))


def shows(number, seen):
    """GitHub records a push on its pull request a moment after it takes the push."""
    for attempt in range(POLLS):
        if attempt:
            time.sleep(POLL_SECONDS)
        if seen(pull_request(number)):
            return True
    return False


def post_status(sha, state, description):
    gh(
        'api',
        f'repos/{{owner}}/{{repo}}/statuses/{sha}',
        '-f',
        f'state={state}',
        '-f',
        f'context={STATUS_CONTEXT}',
        '-f',
        f'description={description}',
    )


def comment(number, body):
    gh('pr', 'comment', str(number), '--body', body)
