"""Worktree and build-directory cleanup: every rule that keeps work, and the removals."""

import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from cycling_devtools import clean

HOUR = 3600


def git(cwd, *args):
    subprocess.run(
        ['git', '-c', 'user.name=t', '-c', 'user.email=t@t', *args],
        cwd=cwd,
        check=True,
        capture_output=True,
    )


class CleanCase(unittest.TestCase):
    """A primary checkout with a bare origin, plus a fake /proc."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        base = Path(temporary.name).resolve()
        self.origin = base / 'origin.git'
        git(base, 'init', '-q', '--bare', '-b', 'main', str(self.origin))
        self.primary = base / 'primary'
        git(base, 'clone', '-q', str(self.origin), str(self.primary))
        (self.primary / '.gitignore').write_text('/.local/\ntodo.md\n/target/\n__pycache__/\n')
        git(self.primary, 'add', '.')
        git(self.primary, 'commit', '-q', '-m', 'first')
        git(self.primary, 'push', '-q', 'origin', 'HEAD:main')
        git(self.primary, 'fetch', '-q', 'origin')
        self.base = base
        self.proc = base / 'proc'
        self.proc.mkdir()
        self.occupy(os.getpid(), base)

    def occupy(self, pid, cwd, command='bash'):
        entry = self.proc / str(pid)
        entry.mkdir(exist_ok=True)
        (entry / 'cwd').unlink(missing_ok=True)
        (entry / 'cwd').symlink_to(cwd)
        (entry / 'cmdline').write_bytes(command.encode())

    def add_worktree(self, name, commit=False):
        path = self.base / name
        git(self.primary, 'worktree', 'add', '-q', '-b', name, str(path), 'origin/main')
        if commit:
            (path / f'{name}.txt').write_text(name)
            git(path, 'add', '.')
            git(path, 'commit', '-q', '-m', name)
        return path.resolve()

    def ctx(self, current=None, idle_hours=0.0):
        return clean.context(idle_hours, current or self.base, self.primary, self.proc)

    def tree(self, path, ctx=None):
        return next(tree for tree in (ctx or self.ctx()).trees if tree.path == Path(path).resolve())

    def judge(self, path, current=None, pull_request=None, idle_hours=0.0):
        ctx = self.ctx(current, idle_hours)
        with patch.object(clean, 'merged_pull_request', return_value=pull_request):
            return clean.judge(self.tree(path, ctx), ctx)

    def merge_into_main(self, path):
        git(path, 'push', '-q', 'origin', 'HEAD:main')
        git(self.primary, 'fetch', '-q', 'origin')

    def age(self, path, hours):
        stamp = time.time() - hours * HOUR
        for entry in [path, *path.rglob('*')]:
            os.utime(entry, (stamp, stamp), follow_symlinks=False)


R, K = clean.Action.REMOVE, clean.Action.KEEP


class ProcessTests(CleanCase):
    def test_a_cwd_inside_a_worktree_counts_for_it_and_every_worktree_around_it(self):
        inner = self.add_worktree('inner')
        (inner / 'src').mkdir()
        self.occupy(11, inner / 'src', 'cargo build')
        ctx = self.ctx()
        self.assertEqual([p.pid for p in clean.occupants(inner, ctx)], [11])
        self.assertEqual([p.pid for p in clean.occupants(self.primary, ctx)], [])
        self.occupy(11, inner / 'src')
        self.assertEqual([p.pid for p in clean.direct_occupants(self.tree(inner, ctx), ctx)], [11])

    def test_a_deleted_cwd_still_counts(self):
        inner = self.add_worktree('inner')
        entry = self.proc / '12'
        entry.mkdir()
        (entry / 'cwd').symlink_to(f'{inner} (deleted)')
        (entry / 'cmdline').write_bytes(b'x')
        self.assertTrue(clean.occupants(inner, self.ctx()))

    def test_unreadable_proc_fails_closed(self):
        with self.assertRaisesRegex(RuntimeError, 'live agents cannot be detected'):
            clean.processes(self.base / 'no-proc')


class JudgeTests(CleanCase):
    def merged_worktree(self, name='merged'):
        path = self.add_worktree(name, commit=True)
        self.merge_into_main(path)
        return path

    def test_primary_and_current_are_kept(self):
        current = self.add_worktree('current')
        self.assertEqual(self.judge(self.primary).reason, 'primary checkout')
        self.assertEqual(self.judge(current, current=current).reason, 'current worktree')

    def test_a_live_process_keeps_even_a_merged_clean_worktree(self):
        merged = self.merged_worktree()
        self.occupy(21, merged)
        decision = self.judge(merged, pull_request=7)
        self.assertEqual(decision.action, K)
        self.assertIn('live process 21', decision.reason)

    def test_an_agent_inside_a_nested_worktree_protects_the_outer_one(self):
        outer = self.merged_worktree('outer')
        inner = outer / 'inner'
        git(self.primary, 'worktree', 'add', '-q', '-b', 'inner', str(inner), 'origin/main')
        self.occupy(22, inner)
        self.assertEqual(self.judge(outer).action, K)
        self.assertIn('live process 22', self.judge(outer).reason)
        self.proc.joinpath('22', 'cwd').unlink()
        self.assertIn('contains worktree inner', self.judge(outer).reason)

    def test_uncommitted_and_untracked_changes_keep(self):
        edited, untracked = self.add_worktree('edited'), self.add_worktree('untracked')
        (edited / '.gitignore').write_text('changed')
        (untracked / 'notes.txt').write_text('work')
        for path in (edited, untracked):
            decision = self.judge(path, pull_request=7)
            self.assertEqual((decision.action, decision.reason), (K, 'uncommitted or untracked changes'))

    def test_ignored_files_outside_the_build_dirs_keep_the_worktree(self):
        for relative in (
            'todo.md',
            '.local/harness/run-1/log',
            '.local/sim/live/session',
            '.local/sim/compare/base-src/x',
        ):
            with self.subTest(relative=relative):
                path = self.merged_worktree(f'ignored-{abs(hash(relative))}')
                target = path / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text('precious')
                decision = self.judge(path, pull_request=7)
                self.assertEqual(decision.action, K)
                self.assertIn('ignored files outside build dirs', decision.reason)

    def test_regenerable_ignored_output_does_not_block_removal(self):
        built = self.merged_worktree('built')
        for relative in (
            '.local/firmware/cycling-os',
            '.local/verify/log',
            '.local/sim/latest/a.png',
            'target/x',
            'a/__pycache__/m.pyc',
        ):
            (built / relative).parent.mkdir(parents=True, exist_ok=True)
            (built / relative).write_text('x')
        self.assertEqual(self.judge(built).action, R)

    def test_a_worktree_with_recent_edits_is_kept_until_it_has_been_quiet(self):
        merged = self.merged_worktree()
        self.assertEqual(self.judge(merged, idle_hours=24).action, K)
        self.assertIn('under 24 h', self.judge(merged, idle_hours=24).reason)
        self.age(merged, 30)
        self.assertEqual(self.judge(merged, idle_hours=24).action, R)

    def test_a_locked_worktree_is_kept(self):
        locked = self.merged_worktree()
        git(self.primary, 'worktree', 'lock', str(locked))
        self.assertEqual(self.judge(locked).reason, 'locked')

    def test_a_worktree_without_commits_of_its_own_is_kept(self):
        decision = self.judge(self.add_worktree('fresh'))
        self.assertEqual(decision.action, K)
        self.assertIn('no commits of its own', decision.reason)

    def test_head_in_origin_main_with_own_commits_is_merged(self):
        decision = self.judge(self.merged_worktree())
        self.assertEqual(decision.action, R)
        self.assertFalse(decision.branch_force)

    def test_unmerged_commits_without_a_merged_pr_are_kept(self):
        self.assertEqual(self.judge(self.add_worktree('wip', commit=True)).reason, 'not merged')

    def test_a_merged_pr_removes_and_forces_the_squashed_branch(self):
        decision = self.judge(self.add_worktree('squashed', commit=True), pull_request=42)
        self.assertEqual(decision.action, R)
        self.assertIn('PR #42', decision.reason)
        self.assertTrue(decision.branch_force)

    def test_a_missing_directory_is_kept(self):
        gone = self.add_worktree('gone')
        subprocess.run(['rm', '-rf', str(gone)], check=True)
        self.assertEqual(self.judge(gone).action, K)


class MergedPullRequestTests(CleanCase):
    def fake_gh(self, payload):
        real = clean.run

        def run(*command, cwd=clean.ROOT):
            if command[0] == 'gh':
                return subprocess.CompletedProcess(command, 0, payload, '')
            return real(*command, cwd=cwd)

        return patch.object(clean, 'run', run)

    def head(self, path):
        return subprocess.run(
            ['git', 'rev-parse', 'HEAD'], cwd=path, capture_output=True, text=True, check=True
        ).stdout.strip()

    def test_matches_the_tip_or_a_commit_inside_it(self):
        path = self.add_worktree('topic', commit=True)
        head = self.head(path)
        with self.fake_gh(f'[{{"number": 5, "headRefOid": "{head}"}}]'):
            self.assertEqual(clean.merged_pull_request('topic', head, self.primary), 5)

    def test_commits_added_after_the_merge_do_not_count(self):
        path = self.add_worktree('topic', commit=True)
        merged_tip = self.head(path)
        (path / 'later.txt').write_text('new work')
        git(path, 'add', '.')
        git(path, 'commit', '-q', '-m', 'later')
        with self.fake_gh(f'[{{"number": 5, "headRefOid": "{merged_tip}"}}]'):
            self.assertIsNone(clean.merged_pull_request('topic', self.head(path), self.primary))

    def test_gh_failure_and_invalid_json_are_not_merged(self):
        failed = subprocess.CompletedProcess([], 1, '', 'no network')
        with patch.object(clean, 'run', return_value=failed):
            self.assertIsNone(clean.merged_pull_request('topic', 'abc', self.primary))
        for payload in ('<html>rate limited', '{"number": 1}', '[{"number": 1}]', '[1]'):
            with self.subTest(payload=payload), self.fake_gh(payload):
                self.assertIsNone(clean.merged_pull_request('topic', 'abc', self.primary))


class IdleBuildDirTests(CleanCase):
    def make_dirs(self, tree):
        for name in ('firmware', 'firmware-sdk', 'sim', 'elf'):
            (tree / '.local' / name).mkdir(parents=True, exist_ok=True)
            (tree / '.local' / name / 'blob').write_bytes(b'x' * 10_000)

    def rows(self, tree, hours=24, procs=()):
        ctx = self.ctx(idle_hours=hours)
        ctx = clean.Context(ctx.trees, [*ctx.processes, *procs], ctx.current, ctx.root, hours, time.time())
        owner = self.tree(tree, ctx)
        return {d.name: clean.judge_dir(owner, d, ctx)[1] for d in clean.build_dirs(owner)}

    def test_only_dirs_idle_past_the_threshold_go_and_sim_and_elf_are_never_listed(self):
        kept = self.add_worktree('kept')
        self.make_dirs(kept)
        self.age(kept / '.local/firmware', 30)
        rows = self.rows(kept)
        self.assertEqual(set(rows), {'firmware', 'firmware-sdk'})
        self.assertEqual(rows['firmware'].action, R)
        self.assertEqual(rows['firmware-sdk'].action, K)

    def test_one_fresh_file_deep_inside_keeps_the_directory(self):
        kept = self.add_worktree('kept')
        self.make_dirs(kept)
        deep = kept / '.local/firmware/release/deps'
        deep.mkdir(parents=True)
        (deep / 'fresh').write_text('x')
        self.age(kept / '.local/firmware', 30)
        (deep / 'fresh').touch()
        self.assertEqual(self.rows(kept)['firmware'].action, K)

    def test_threshold_is_configurable(self):
        kept = self.add_worktree('kept')
        self.make_dirs(kept)
        self.age(kept / '.local/firmware', 5)
        self.assertEqual(self.rows(kept, hours=24)['firmware'].action, K)
        self.assertEqual(self.rows(kept, hours=4)['firmware'].action, R)

    def test_a_live_process_keeps_idle_dirs(self):
        kept = self.add_worktree('kept')
        self.make_dirs(kept)
        self.age(kept / '.local/firmware', 100)
        busy = clean.Process(1, 'cargo', kept)
        self.assertEqual(self.rows(kept, procs=[busy])['firmware'].action, K)

    def test_hard_links_count_once_and_symlinked_dirs_are_skipped(self):
        kept = self.add_worktree('kept')
        (kept / '.local/firmware').mkdir(parents=True)
        (kept / '.local/firmware/a').write_bytes(b'x' * 100_000)
        os.link(kept / '.local/firmware/a', kept / '.local/firmware/b')
        (kept / '.local/verify').symlink_to(self.primary, target_is_directory=True)
        size, _ = clean.measure(kept / '.local/firmware')
        self.assertLess(size, 150_000)
        self.assertEqual(set(self.rows(kept)), {'firmware'})


class ApplyTests(CleanCase):
    def row(self, kind, path, branch=None, force=False):
        return clean.Row(kind, path, 0, R, 'x', branch, force)

    def test_removes_the_worktree_and_its_merged_branch(self):
        merged = self.add_worktree('merged', commit=True)
        self.merge_into_main(merged)
        self.assertIsNone(clean.apply_row(self.row('worktree', merged, 'merged'), self.primary))
        self.assertFalse(merged.exists())
        branches = subprocess.run(
            ['git', 'branch', '--list', 'merged'], cwd=self.primary, capture_output=True, text=True
        ).stdout
        self.assertEqual(branches, '')

    def test_dirty_worktree_is_refused_by_git_and_kept(self):
        dirty = self.add_worktree('dirty')
        (dirty / 'work.txt').write_text('precious')
        self.assertIsNotNone(clean.apply_row(self.row('worktree', dirty, 'dirty'), self.primary))
        self.assertEqual((dirty / 'work.txt').read_text(), 'precious')

    def test_an_unmerged_branch_survives(self):
        wip = self.add_worktree('wip', commit=True)
        self.assertIn('branch kept', clean.apply_row(self.row('worktree', wip, 'wip'), self.primary))

    def test_build_dir_removal_refuses_paths_outside_dot_local(self):
        kept = self.add_worktree('kept')
        (kept / 'src').mkdir()
        self.assertIn('refusing', clean.apply_row(self.row('build dir', kept / 'src')))
        self.assertTrue((kept / 'src').exists())
        (kept / '.local').mkdir()
        (kept / '.local/link').symlink_to(kept / 'src', target_is_directory=True)
        self.assertIn('refusing', clean.apply_row(self.row('build dir', kept / '.local/link')))
        self.assertTrue((kept / 'src').exists())

    def test_removes_an_idle_build_dir(self):
        kept = self.add_worktree('kept')
        (kept / '.local/firmware').mkdir(parents=True)
        self.assertIsNone(clean.apply_row(self.row('build dir', kept / '.local/firmware')))
        self.assertFalse((kept / '.local/firmware').exists())


class PlanTests(CleanCase):
    def test_plan_removes_only_dead_worktrees_and_never_the_primary_or_current(self):
        dead = self.add_worktree('dead', commit=True)
        self.merge_into_main(dead)
        busy = self.add_worktree('busy', commit=True)
        self.merge_into_main(busy)
        wip = self.add_worktree('wip', commit=True)
        mine = self.add_worktree('mine')
        self.occupy(31, busy)
        for tree in (self.primary, wip):
            (tree / '.local/firmware').mkdir(parents=True)
            self.age(tree / '.local/firmware', 50)
        with patch.object(clean, 'merged_pull_request', return_value=None):
            rows = clean.plan(self.ctx(current=mine))
        removed = {row.path for row in clean.removable(rows)}
        self.assertEqual(removed, {dead, wip / '.local/firmware', self.primary.resolve() / '.local/firmware'})

    def test_a_row_is_judged_again_before_it_is_deleted(self):
        dead = self.add_worktree('dead', commit=True)
        self.merge_into_main(dead)
        with patch.object(clean, 'merged_pull_request', return_value=None):
            row = next(r for r in clean.plan(self.ctx()) if r.path == dead)
            self.assertEqual(row.action, R)
            self.occupy(41, dead)
            self.assertIn('live process 41', clean.rejudge(row, self.ctx()).reason)
            (self.proc / '41' / 'cwd').unlink()
            (dead / 'new.txt').write_text('started work')
            self.assertEqual(clean.rejudge(row, self.ctx()).reason, 'uncommitted or untracked changes')

    def test_main_prints_a_dry_run_table_and_removes_nothing(self):
        dead = self.add_worktree('dead')
        rows = [clean.Row('worktree', dead, 0, R, 'why')]
        with patch.object(clean, 'plan', return_value=rows), patch.object(clean, 'context'):
            self.assertEqual(clean.main(['--no-fetch']), 0)
        self.assertTrue(dead.exists())


class DiskGuardTests(unittest.TestCase):
    def test_warns_under_the_threshold_and_names_the_command(self):
        with patch.object(clean, 'free_gb', return_value=12.0):
            self.assertIn('mise run clean', clean.low_disk_warning())

    def test_rounds_down_so_a_shortfall_never_prints_the_threshold(self):
        with patch.object(clean, 'free_gb', return_value=29.6):
            self.assertIn('only 29 GB', clean.low_disk_warning())

    def test_silent_with_enough_space(self):
        with patch.object(clean, 'free_gb', return_value=31.0):
            self.assertIsNone(clean.low_disk_warning())


if __name__ == '__main__':
    unittest.main()
