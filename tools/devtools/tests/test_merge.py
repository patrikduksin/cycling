import dataclasses
import subprocess
import time
import types

import pytest

from cycling_devtools import clean, forge, lanes, merge, merge_checkout, review, slow_tier, verify
from verify_fixture import commit, gate_runs, git

PR = forge.PullRequest(7, 'feature', 'main', 'Add b', 'Because b was missing.', 'OPEN', False, '')
RECEIVED = '#!/bin/sh\n{ cat; echo; } >> pushes.log\n'


class GitHub:
    def __init__(self, monkeypatch, root, pr=PR):
        self.root = root
        self.remote = root.parent / 'origin.git'
        self.pr = pr
        self.statuses = []
        self.comments = []
        self.started = []
        self.before_status = lambda: None
        self.shows_heads = True
        self.shows_merges = True
        self.merged = False
        self.head = git('ls-remote', 'origin', f'refs/heads/{pr.branch}', cwd=root).split('\t')[0]
        hook = self.remote / 'hooks' / 'post-receive'
        hook.write_text(RECEIVED)
        hook.chmod(0o755)
        self.seen = len(self.pushes())
        monkeypatch.setattr(forge, 'pull_request', self.pull_request)
        monkeypatch.setattr(forge, 'post_status', self.post_status)
        monkeypatch.setattr(forge, 'comment', self.comment)
        monkeypatch.setattr(forge, 'POLL_SECONDS', 0)
        monkeypatch.setattr(merge, 'detached', lambda command, log, cwd: self.started.append((command, log.name)))
        # The gates that start the background jobs; a path no rule matches selects them all.
        monkeypatch.setattr(lanes, 'EVERY', lanes.EVERY | {'screens', 'test-web', 'test-rust'})

    def pushes(self):
        log = self.remote / 'pushes.log'
        return log.read_text().split('\n\n')[:-1] if log.exists() else []

    def holds(self, commit, tip=None):
        check = ['cat-file', '-e', commit] if tip is None else ['merge-base', '--is-ancestor', commit, tip]
        return subprocess.run(['git', *check], cwd=self.remote, capture_output=True).returncode == 0

    def receive(self):
        pushes = self.pushes()
        for push in pushes[self.seen :]:
            moved = {ref: new for _, new, ref in map(str.split, push.splitlines())}
            main = moved.get('refs/heads/main')
            if main and self.head and self.holds(self.head, main):
                self.merged = True
            head = moved.get(f'refs/heads/{self.pr.branch}')
            if head and self.shows_heads and set(head) != {'0'}:
                self.head = head
        self.seen = len(pushes)

    def pull_request(self, number):
        self.receive()
        state = 'MERGED' if self.merged and self.shows_merges else self.pr.state
        return dataclasses.replace(self.pr, state=state, head=self.head)

    def post_status(self, sha, state, description):
        assert self.holds(sha), 'GitHub takes a status only for a commit it holds'
        self.before_status()
        self.statuses.append((sha, state))

    def comment(self, number, body):
        self.comments.append(body)


@pytest.fixture
def github(monkeypatch, checkout):
    return GitHub(monkeypatch, checkout)


@pytest.fixture
def feature(checkout):
    git('checkout', '-q', '-b', 'feature', cwd=checkout)
    commit(checkout, 'Add b', **{'src/b.txt': 'b\n'})
    head = commit(checkout, 'Fix b', **{'src/b.txt': 'better b\n'})
    git('push', '-q', 'origin', 'feature', cwd=checkout)
    git('checkout', '-q', 'main', cwd=checkout)
    return head


@pytest.fixture
def other(tmp_path):
    clone = tmp_path / 'other'
    git('clone', '-q', str(tmp_path / 'origin.git'), str(clone), cwd=tmp_path)
    return clone


def origin(ref, root):
    return git('ls-remote', 'origin', ref, cwd=root).split()[0]


def push_to(branch, root, message, **files):
    git('checkout', '-q', branch, cwd=root)
    git('pull', '-q', '--ff-only', 'origin', branch, cwd=root)
    head = commit(root, message, **files)
    git('push', '-q', 'origin', branch, cwd=root)
    return head


def approve(head, by='thermos-bugs/opus', verdict='PASS'):
    review.record(review.Verdict(review.patch_id(head, 'origin/main'), by, verdict, time.time(), 12.0))


def during_verify(monkeypatch, happen):
    verified = verify.verify

    def verifying(request):
        outcome = verified(request)
        happen()
        return outcome

    monkeypatch.setattr(verify, 'verify', verifying)


def test_a_reviewed_pull_request_lands_as_the_one_verified_commit(checkout, github, feature):
    start = origin('main', checkout)
    approve(feature)

    merge.merge(7)

    landed = origin('main', checkout)
    assert github.statuses == [(landed, 'success')]
    assert git('rev-parse', f'{landed}^@', cwd=checkout) == start
    assert git('show', f'{landed}:src/b.txt', cwd=checkout) == 'better b'
    assert git('log', '-1', '--format=%s%n%n%b', landed, cwd=checkout) == 'Add b (#7)\n\nBecause b was missing.'
    assert github.merged
    assert git('ls-remote', 'origin', 'feature', cwd=checkout) == ''
    assert gate_runs(checkout) == 1
    assert f'Merged as {landed}' in github.comments[0]
    assert '| thermos-bugs/opus | PASS | 12 s |' in github.comments[0]


def test_the_status_is_posted_once_the_branch_holds_the_squash_and_before_main_moves(checkout, github, feature):
    approve(feature)
    seen = {}
    github.before_status = lambda: seen.update(branch=origin('feature', checkout), main=origin('main', checkout))
    start = origin('main', checkout)

    merge.merge(7)

    assert seen == {'branch': origin('main', checkout), 'main': start}


def test_a_squash_github_never_shows_as_the_head_does_not_land_and_the_branch_goes_back(
    checkout, github, feature, capsys
):
    approve(feature)
    start = origin('main', checkout)
    github.shows_heads = False

    with pytest.raises(SystemExit, match=r'did not land[\s\S]*GitHub did not show \w+ as the head of #7'):
        merge.merge(7)

    assert (origin('main', checkout), origin('feature', checkout)) == (start, feature)
    assert f'Its branch feature is back at {feature}.' in capsys.readouterr().out
    assert github.started == []


def test_a_fail_recorded_while_verify_runs_stops_the_merge(checkout, github, feature, monkeypatch):
    approve(feature)
    start = origin('main', checkout)
    during_verify(monkeypatch, lambda: approve(feature, by='thermos-quality/opus', verdict='FAIL'))

    with pytest.raises(SystemExit, match='needs a review PASS'):
        merge.merge(7)

    assert (origin('main', checkout), origin('feature', checkout)) == (start, feature)
    assert github.statuses == []


def test_a_pass_on_a_stacked_pull_request_does_not_cover_what_its_parent_dropped_before_landing(
    checkout, monkeypatch, capsys
):
    git('checkout', '-q', '-b', 'below', cwd=checkout)
    commit(checkout, 'Parent', **{'src/p.txt': 'p\n', 'src/debug.py': 'print()\n'})
    git('push', '-q', 'origin', 'below', cwd=checkout)
    git('checkout', '-q', '-b', 'feature', cwd=checkout)
    commit(checkout, 'Child', **{'src/b.txt': 'b\n'})
    git('push', '-q', 'origin', 'feature', cwd=checkout)
    github = GitHub(monkeypatch, checkout, dataclasses.replace(PR, base='below'))
    assert reviewing('record', '--patch', reviewed_patch(capsys), '--by', 'thermos-bugs/opus', '--verdict', 'PASS') == 0

    git('checkout', '-q', 'main', cwd=checkout)
    commit(checkout, 'Parent, as reviewed (#6)', **{'src/p.txt': 'p\n'})
    git('push', '-q', 'origin', 'main', cwd=checkout)
    github.pr = PR
    start = origin('main', checkout)

    with pytest.raises(SystemExit, match='needs a review PASS'):
        merge.merge(7)
    assert origin('main', checkout) == start

    git('rebase', '-q', '--onto', 'main', 'below', 'feature', cwd=checkout)
    git('push', '-q', '--force', 'origin', 'feature', cwd=checkout)
    merge.merge(7)
    assert git('ls-tree', '-r', '--name-only', origin('main', checkout), 'src', cwd=checkout).splitlines() == [
        'src/a.txt',
        'src/b.txt',
        'src/p.txt',
    ]


def test_a_merge_that_cannot_update_its_pull_request_is_still_a_merge_with_its_background_jobs(
    checkout, github, feature, monkeypatch, capsys
):
    approve(feature)

    def unreachable(number, body):
        raise subprocess.CalledProcessError(1, 'gh', stderr='HTTP 502')

    monkeypatch.setattr(forge, 'comment', unreachable)

    merge.merge(7)

    landed = origin('main', checkout)
    said = capsys.readouterr().out
    assert f'merged #7 as {landed}' in said
    assert '#7 is merged, but its pull request was not updated: HTTP 502' in said
    assert [log for _, log in github.started] == ['fill.log', 'slow-tier.log', 'deep.log']


def test_a_merge_of_paths_that_select_no_gate_starts_no_background_job(checkout, github, monkeypatch):
    git('checkout', '-q', '-b', 'feature', cwd=checkout)
    commit(checkout, 'Docs', **{'docs/b.md': 'b\n'})
    git('push', '-q', 'origin', 'feature', cwd=checkout)
    git('checkout', '-q', 'main', cwd=checkout)
    head = git('rev-parse', 'feature', cwd=checkout)
    github.head = head
    approve(head)

    merge.merge(7)

    assert github.merged and github.started == []


def test_the_squash_is_never_dated_before_the_main_it_lands_on(checkout, feature, monkeypatch):
    monkeypatch.setenv('GIT_COMMITTER_DATE', '2099-01-01T00:00:00+01:00')
    later = push_to('main', checkout, 'Later', **{'src/later.txt': 'l\n'})
    monkeypatch.delenv('GIT_COMMITTER_DATE')

    squash = merge.squash(forge.fetch(PR), PR)

    assert git('log', '-1', '--format=%aI', squash, cwd=checkout) == git(
        'log', '-1', '--format=%aI', feature, cwd=checkout
    )
    assert git('log', '-1', '--format=%cI', squash, cwd=checkout) == git(
        'log', '-1', '--format=%cI', later, cwd=checkout
    )


def test_the_screens_are_cached_and_the_slow_and_deep_tiers_started_for_the_landed_commit(checkout, github, feature):
    approve(feature)
    merge.merge(7)
    landed = origin('main', checkout)
    fill, slow, deep = github.started
    assert fill == (['nice', '-n', '19', 'cycling', 'sim-compare', '--fill', landed], 'fill.log')
    assert slow[0][-2:] == ['7', landed] and slow[1] == 'slow-tier.log'
    assert deep == (merge.DEEP_TIER, 'deep.log')


def test_a_background_job_adds_to_its_log(checkout, tmp_path):
    for word in ('first', 'second'):
        merge.detached(['echo', word], tmp_path / 'logs' / 'job.log', checkout).wait()
    assert (tmp_path / 'logs' / 'job.log').read_text() == 'first\nsecond\n'


def test_a_pull_request_without_a_pass_on_its_patch_is_refused(checkout, github, feature):
    start = origin('main', checkout)
    with pytest.raises(SystemExit, match='needs a review PASS'):
        merge.merge(7)
    approve(feature)
    approve(feature, by='thermos-quality/opus', verdict='FAIL')
    with pytest.raises(SystemExit, match='needs a review PASS'):
        merge.merge(7)
    assert origin('main', checkout) == start
    assert gate_runs(checkout) == 0


def test_a_pass_does_not_cover_a_later_edit_to_the_pull_request(checkout, github, feature):
    approve(feature)
    push_to('feature', checkout, 'One more thing', **{'src/c.txt': 'c\n'})
    with pytest.raises(SystemExit, match='needs a review PASS'):
        merge.merge(7)


def test_a_pull_request_stacked_on_another_is_refused(checkout, monkeypatch, feature):
    GitHub(monkeypatch, checkout, dataclasses.replace(PR, base='s1/below'))
    approve(feature)
    with pytest.raises(SystemExit, match='targets s1/below, not main'):
        merge.merge(7)
    assert gate_runs(checkout) == 0


def test_a_tree_that_fails_verify_never_reaches_main(checkout, github, feature):
    head = push_to('feature', checkout, 'Break it', broken='')
    approve(head)
    start = origin('main', checkout)

    with pytest.raises(SystemExit, match='fails verify'):
        merge.merge(7)

    assert origin('main', checkout) == start
    assert origin('feature', checkout) == head
    assert github.statuses == []


def test_prepare_verifies_the_squash_so_the_merge_finds_it_cached(checkout, github, feature):
    start = origin('main', checkout)
    merge.prepare(7)
    assert (origin('main', checkout), origin('feature', checkout)) == (start, feature)
    assert gate_runs(checkout) == 1
    approve(feature)

    merge.merge(7)

    assert gate_runs(checkout) == 1
    assert '| `probe` | ✅ | cached' in github.comments[0]


def test_the_same_main_and_head_always_squash_to_the_same_commit(checkout, feature):
    heads = forge.fetch(PR)
    first = merge.squash(heads, PR)
    time.sleep(1.1)
    assert merge.squash(heads, PR) == first


def test_the_merge_checkout_is_a_locked_worktree_that_cycling_clean_leaves_alone(checkout, feature):
    place = merge_checkout.checkout('verify', feature)
    assert (place / 'src' / 'b.txt').read_text() == 'better b\n'
    listed = git('worktree', 'list', '--porcelain', cwd=checkout)
    assert f'worktree {place}' in listed and 'locked cycling merge' in listed
    assert [tree.path for tree in clean.worktrees(checkout)] == [checkout.resolve()]
    git('worktree', 'prune', cwd=checkout)
    git('gc', '-q', '--prune=now', cwd=checkout)
    assert git('status', '--porcelain', cwd=place) == ''


def test_the_next_merge_checkout_rewrites_only_the_files_that_changed(checkout, feature):
    place = merge_checkout.checkout('verify', feature)
    written = {name: (place / 'src' / name).stat().st_mtime_ns for name in ('a.txt', 'b.txt')}
    (place / '.local').mkdir()
    (place / 'stray.txt').write_text('left by a gate')
    merge_checkout.checkout('verify', git('rev-parse', 'feature^', cwd=checkout))
    assert (place / 'src' / 'a.txt').stat().st_mtime_ns == written['a.txt']
    assert (place / 'src' / 'b.txt').stat().st_mtime_ns != written['b.txt']
    assert (place / 'src' / 'b.txt').read_text() == 'b\n'
    assert (place / '.local').is_dir()
    assert not (place / 'stray.txt').exists()


def test_when_main_moves_under_the_merge_the_new_merge_tree_is_verified_and_lands(checkout, github, feature, other):
    approve(feature)

    def main_moves():
        if not github.statuses:
            push_to('main', other, 'Meanwhile', **{'src/meanwhile.txt': 'm\n'})

    github.before_status = main_moves

    merge.merge(7)

    landed = origin('main', checkout)
    moved = git('rev-parse', 'HEAD', cwd=other)
    assert git('rev-parse', f'{landed}^@', cwd=checkout) == moved
    assert github.statuses[-1] == (landed, 'success')
    assert git('show', f'{landed}:src/meanwhile.txt', cwd=checkout) == 'm'
    assert git('show', f'{landed}:src/b.txt', cwd=checkout) == 'better b'
    assert gate_runs(checkout) == 2


def test_a_pass_survives_main_changing_the_lines_beside_the_patch_and_then_moving_under_the_merge(
    checkout, github, other
):
    lines = [f'line {number}\n' for number in range(12)]
    push_to('main', checkout, 'A longer file', **{'src/long.txt': ''.join(lines)})
    git('checkout', '-q', '-b', 'feature', cwd=checkout)
    head = commit(checkout, 'Change line 6', **{'src/long.txt': ''.join(lines).replace('line 6\n', 'six\n')})
    git('push', '-q', 'origin', 'feature', cwd=checkout)
    approve(head)
    push_to('main', other, 'Beside it', **{'src/long.txt': ''.join(lines).replace('line 4\n', 'four\n')})

    def main_moves():
        if not github.statuses:
            push_to('main', other, 'Meanwhile', **{'src/meanwhile.txt': 'm\n'})

    github.before_status = main_moves

    merge.merge(7)

    landed = origin('main', checkout)
    assert git('show', f'{landed}:src/long.txt', cwd=checkout).splitlines()[4:7] == ['four', 'line 5', 'six']
    assert git('show', f'{landed}:src/meanwhile.txt', cwd=checkout) == 'm'


def test_a_head_pushed_under_the_merge_stops_it_with_nothing_landed_and_the_branch_kept(
    checkout, github, feature, other, monkeypatch
):
    approve(feature)
    start = origin('main', checkout)
    during_verify(monkeypatch, lambda: push_to('feature', other, 'Late', **{'src/late.txt': 'late\n'}))

    with pytest.raises(SystemExit, match='changed during the merge'):
        merge.merge(7)

    assert origin('main', checkout) == start
    assert origin('feature', checkout) == git('rev-parse', 'HEAD', cwd=other)
    assert github.statuses == []


def test_a_head_pushed_after_github_shows_the_squash_keeps_main_and_the_new_head(
    checkout, github, feature, other, monkeypatch, capsys
):
    approve(feature)
    start = origin('main', checkout)
    real = merge.push
    late = []

    def push(*refspecs):
        if any(spec.endswith(':refs/heads/main') for spec in refspecs) and not late:
            git('fetch', '-q', 'origin', cwd=other)
            git('checkout', '-q', '-B', 'feature', 'origin/feature', cwd=other)
            late.append(commit(other, 'Late', **{'src/late.txt': 'late\n'}))
            git('push', '-q', 'origin', 'feature', cwd=other)
        return real(*refspecs)

    monkeypatch.setattr(merge, 'push', push)
    with pytest.raises(SystemExit, match='changed during the merge'):
        merge.merge(7)

    assert origin('main', checkout) == start
    assert origin('feature', checkout) == late[0]
    assert 'did not land' in capsys.readouterr().out


def test_a_head_pushed_in_the_moment_main_lands_stays_on_its_branch_and_a_rerun_converges(
    checkout, github, feature, other, monkeypatch, capsys
):
    approve(feature)
    real = merge.push
    late = []

    def push(*refspecs):
        if '--atomic' not in refspecs:
            return real(*refspecs)
        # As git does when the branch moves after origin advertised it: the unchanged branch is not sent.
        git('fetch', '-q', 'origin', cwd=other)
        git('checkout', '-q', '-B', 'feature', 'origin/feature', cwd=other)
        late.append(commit(other, 'Late', **{'src/late.txt': 'late\n'}))
        git('push', '-q', 'origin', 'feature', cwd=other)
        return real(*(spec for spec in refspecs if 'feature' not in spec and spec != '--atomic'))

    monkeypatch.setattr(merge, 'push', push)
    merge.merge(7)

    landed = origin('main', checkout)
    assert not github.holds(late[0], landed), 'only the reviewed squash reaches main'
    assert origin('feature', checkout) == late[0]
    assert not github.merged
    assert 'branch `feature` is kept' in github.comments[-1]
    with pytest.raises(SystemExit, match='needs a review PASS'):
        merge.merge(7)
    assert origin('main', checkout) == landed


def test_a_main_push_origin_applied_but_reported_as_failed_is_a_merge(checkout, github, feature, monkeypatch, capsys):
    approve(feature)
    real = merge.push

    def push(*refspecs):
        done = real(*refspecs)
        if any(spec.endswith(':refs/heads/main') for spec in refspecs):
            return subprocess.CompletedProcess(done.args, 128, '', 'fatal: the remote end hung up unexpectedly')
        return done

    monkeypatch.setattr(merge, 'push', push)
    merge.merge(7)

    landed = origin('main', checkout)
    assert landed != feature and github.merged
    assert f'merged #7 as {landed}' in capsys.readouterr().out
    assert github.started


def test_a_push_origin_refuses_is_reported_as_refused_not_as_main_moving(checkout, github, feature, tmp_path):
    approve(feature)
    hook = tmp_path / 'origin.git' / 'hooks' / 'pre-receive'
    hook.write_text('#!/bin/sh\nif grep -q " refs/heads/main$"; then echo "main is protected" >&2; exit 1; fi\n')
    hook.chmod(0o755)

    with pytest.raises(SystemExit, match=r'did not land[\s\S]*main is protected'):
        merge.merge(7)

    assert origin('feature', checkout) == feature
    assert gate_runs(checkout) == 1


def test_the_branch_is_kept_when_github_does_not_show_the_pull_request_merged(checkout, github, feature):
    approve(feature)
    github.shows_merges = False

    merge.merge(7)

    landed = origin('main', checkout)
    assert origin('feature', checkout) == landed
    assert 'GitHub does not show #7 as merged, so its branch `feature` is kept.' in github.comments[0]


def landed_merge(checkout, github, feature):
    approve(feature)
    merge.merge(7)
    return origin('main', checkout)


def test_the_slow_tier_runs_on_the_merged_tree_and_comments_on_the_pull_request_only_when_it_fails(
    checkout, github, feature, monkeypatch
):
    landed = landed_merge(checkout, github, feature)
    merged = len(github.comments)

    monkeypatch.setattr(slow_tier, 'COMMAND', 'grep -q better src/b.txt')
    assert slow_tier.main(['7', landed]) == 0
    assert len(github.comments) == merged

    monkeypatch.setattr(slow_tier, 'COMMAND', 'echo the full read timed out; false')
    assert slow_tier.main(['7', landed]) == 1
    assert f'The slow tier failed on {landed}' in github.comments[-1]
    assert 'the full read timed out' in github.comments[-1]


def test_a_slow_tier_that_cannot_run_says_so_on_the_pull_request(checkout, github, feature, monkeypatch):
    landed = landed_merge(checkout, github, feature)

    def no_checkout(name, commit):
        raise OSError('no space left on device')

    monkeypatch.setattr(merge_checkout, 'checkout', no_checkout)
    with pytest.raises(OSError):
        slow_tier.main(['7', landed])
    assert f'The slow tier could not run on {landed}' in github.comments[-1]
    assert 'no space left on device' in github.comments[-1]


def test_the_slow_tier_does_not_start_on_a_nearly_full_disk_and_says_so(checkout, github, feature, monkeypatch):
    landed = landed_merge(checkout, github, feature)
    monkeypatch.setattr(slow_tier, 'COMMAND', 'touch ran')
    monkeypatch.setattr(clean, 'free_gb', lambda path: 9.4)

    assert slow_tier.main(['7', landed]) == 1
    assert f'The slow tier did not run on {landed}' in github.comments[-1]
    assert '9 GB free' in github.comments[-1]
    assert not (checkout / '.local' / 'merge' / 'slow-tier' / 'ran').exists()


def test_the_slow_tier_of_a_merge_that_main_has_left_behind_says_so_and_leaves_it_to_the_newer_one(
    checkout, github, feature, monkeypatch, capsys
):
    monkeypatch.setattr(slow_tier, 'COMMAND', 'false')
    assert slow_tier.main(['7', feature]) == 0
    assert github.comments == []
    assert f'main has moved past {feature}' in capsys.readouterr().out


def test_a_conflict_with_main_is_refused(checkout, github, feature):
    approve(feature)
    commit(checkout, 'Main adds b too', **{'src/b.txt': 'another b\n'})
    git('push', '-q', 'origin', 'main', cwd=checkout)
    with pytest.raises(SystemExit, match='conflicts with main'):
        merge.merge(7)


def test_a_merged_pull_request_is_left_alone(checkout, monkeypatch):
    GitHub(monkeypatch, checkout, dataclasses.replace(PR, state='MERGED'))
    merge.merge(7)
    assert gate_runs(checkout) == 0


def test_a_draft_is_refused(checkout, monkeypatch):
    GitHub(monkeypatch, checkout, dataclasses.replace(PR, draft=True))
    with pytest.raises(SystemExit, match='not an open, ready pull request'):
        merge.merge(7)


def reviewing(*words):
    return review.main([*words, '--pr', '7'])


def reviewed_patch(capsys):
    capsys.readouterr()
    assert reviewing('patch') == 0
    return capsys.readouterr().out.strip()


def test_review_status_follows_the_patch_not_the_commit(checkout, github, feature, capsys, monkeypatch):
    patch = reviewed_patch(capsys)
    assert reviewing('status') == 1
    now = time.time()
    monkeypatch.setattr(review, 'time', types.SimpleNamespace(time=lambda: now))
    assert reviewing('step', '--by', 'deslop', '--started', str(now - 30)) == 0
    assert reviewing('status') == 1
    assert reviewing('record', '--patch', patch, '--by', 'thermos-bugs/opus', '--verdict', 'PASS') == 0
    assert reviewing('status') == 0
    assert '| deslop |  | 30 s |' in capsys.readouterr().out

    push_to('main', checkout, 'Main moves', **{'src/elsewhere.txt': 'e\n'})
    git('checkout', '-q', 'feature', cwd=checkout)
    git('rebase', '-q', 'origin/main', cwd=checkout)
    git('push', '-q', '--force', 'origin', 'feature', cwd=checkout)
    assert reviewing('status') == 0

    push_to('feature', checkout, 'Deslop', **{'src/b.txt': 'best b\n'})
    assert reviewing('status') == 1


def test_a_verdict_on_a_patch_the_branch_no_longer_has_is_refused(checkout, github, feature, capsys):
    read = reviewed_patch(capsys)
    push_to('feature', checkout, 'Pushed while the review ran', **{'src/c.txt': 'c\n'})
    with pytest.raises(SystemExit, match=f'the review read patch {read[:12]}'):
        reviewing('record', '--patch', read, '--by', 'thermos-bugs/opus', '--verdict', 'PASS')
    assert reviewing('status') == 1


def test_a_step_recorded_after_a_fail_does_not_erase_it(checkout, github, feature, capsys):
    patch = reviewed_patch(capsys)
    assert reviewing('record', '--patch', patch, '--by', 'thermos-bugs/opus', '--verdict', 'PASS') == 0
    assert reviewing('record', '--patch', patch, '--by', 'thermos-quality/opus', '--verdict', 'FAIL') == 0
    assert reviewing('step', '--by', 'thermos-quality/opus') == 0
    assert reviewing('status') == 1


@pytest.mark.parametrize('setting', [('color.diff', 'always'), ('diff.external', 'echo')])
def test_the_patch_does_not_depend_on_how_this_checkout_shows_a_diff(checkout, feature, setting):
    plain = review.patch_id(feature, 'origin/main')
    git('config', *setting, cwd=checkout)
    assert review.patch_id(feature, 'origin/main') == plain


def test_a_change_of_whitespace_alone_is_a_new_patch(checkout, feature):
    git('checkout', '-q', 'feature', cwd=checkout)
    commit(checkout, 'Python', **{'src/code.py': 'if ready:\n    pass\nstop()\n'})
    before = review.patch_id('HEAD', 'origin/main')
    commit(checkout, 'Indent', **{'src/code.py': 'if ready:\n    pass\n    stop()\n'})
    assert review.patch_id('HEAD', 'origin/main') != before
