import fcntl
import os
import re
import shutil
import subprocess
import tempfile
import time
import unittest
from contextlib import contextmanager
from pathlib import Path

from cycling_devtools import cli
from cycling_devtools.workspace import ROOT

LAUNCHER = ROOT / 'tools' / 'simulator' / 'trace.sh'

FAKE_CARGO = """#!/usr/bin/env bash
set -eu
while [ "$1" != --manifest-path ]; do shift; done
manifest=$(dirname "$2")
mkdir -p "$CARGO_TARGET_DIR/tools"
tree=$(cat "$manifest/apps/app.rs")
commit=$(git -C "$manifest" rev-parse HEAD)
cat > "$CARGO_TARGET_DIR/tools/trace" <<BIN
#!/usr/bin/env bash
if [ "\\$*" = --build-commit ]; then echo "\\${STAMP:-$commit}"; exit; fi
[ -z "\\${PROBE_LOCK:-}" ] || { flock -xn "\\$PROBE_LOCK" true && echo unlocked || echo locked; }
echo "built $commit $tree: \\$*"
exit \\${TRACE_EXIT:-0}
BIN
chmod +x "$CARGO_TARGET_DIR/tools/trace"
echo "$manifest" >> "$BUILDS"
eval "${WHILE_BUILDING:-}"
"""

FAKE_UV = """#!/usr/bin/env bash
echo "uv $*"
exit ${UV_EXIT:-0}
"""


def git(cwd, *args):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


class TraceLauncherTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.repo = self.tmp / 'repo'
        (self.repo / 'tools' / 'simulator').mkdir(parents=True)
        (self.repo / 'apps').mkdir()
        shutil.copy(LAUNCHER, self.repo / 'tools' / 'simulator' / 'trace.sh')
        (self.repo / 'tools' / 'simulator' / 'build-sources').write_text('apps\nCargo.toml\n')
        (self.repo / 'Cargo.toml').write_text('[workspace]\n')
        (self.repo / 'apps' / 'app.rs').write_text('one')
        (self.repo / 'notes.md').write_text('notes')
        git(self.repo, 'init', '-q', '-b', 'main')
        git(self.repo, 'add', '.')
        self.commit('first')
        bin_dir = self.tmp / 'bin'
        bin_dir.mkdir()
        for name, script in (('cargo', FAKE_CARGO), ('uv', FAKE_UV)):
            (bin_dir / name).write_text(script)
            (bin_dir / name).chmod(0o755)
        self.builds = self.tmp / 'builds.log'
        self.builds.touch()
        self.env = {
            **os.environ,
            'PATH': f'{bin_dir}{os.pathsep}{os.environ["PATH"]}',
            'BUILDS': str(self.builds),
            'CARGO_TARGET_DIR': str(self.tmp / 'target'),
        }

    def commit(self, message):
        git(
            self.repo,
            '-c',
            'user.name=test',
            '-c',
            'user.email=test@example.com',
            'commit',
            '-q',
            '-am',
            message,
        )
        return git(self.repo, 'rev-parse', 'HEAD')

    def launch(self, argv, checkout=None, **env):
        return subprocess.run(argv, cwd=checkout or self.repo, env={**self.env, **env}, capture_output=True, text=True)

    def run_trace(self, checkout, *args, **env):
        done = self.launch(['bash', 'tools/simulator/trace.sh', 'trace', *args], checkout, **env)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.strip()

    def built(self):
        return self.builds.read_text().splitlines()

    def cache(self):
        return self.repo / '.local' / 'tools' / 'trace'

    def test_a_clean_commit_builds_once_and_every_worktree_runs_that_binary(self):
        head = git(self.repo, 'rev-parse', 'HEAD')
        self.assertEqual(self.run_trace(self.repo, 'summary', 'x.trace'), f'built {head} one: summary x.trace')
        self.assertTrue((self.cache() / head).is_file())
        self.assertEqual(self.run_trace(self.repo, 'find', 'x.trace'), f'built {head} one: find x.trace')
        worktree = self.tmp / 'worktree'
        git(self.repo, 'worktree', 'add', '-q', '--detach', str(worktree), head)
        self.assertEqual(self.run_trace(worktree, 'timeline'), f'built {head} one: timeline')
        self.assertEqual(len(self.built()), 1, 'built once for the commit')

    def test_an_edit_outside_the_build_sources_still_runs_the_commits_binary(self):
        head = git(self.repo, 'rev-parse', 'HEAD')
        self.run_trace(self.repo, 'summary')
        (self.repo / 'notes.md').write_text('edited')
        self.assertEqual(self.run_trace(self.repo, 'summary'), f'built {head} one: summary')
        self.assertEqual(len(self.built()), 1)

    def test_an_edited_tree_builds_and_runs_its_own_and_leaves_the_cache_alone(self):
        head = git(self.repo, 'rev-parse', 'HEAD')
        self.run_trace(self.repo, 'summary')
        (self.repo / 'apps' / 'app.rs').write_text('two')
        self.assertEqual(self.run_trace(self.repo, 'summary'), f'built {head} two: summary')
        self.assertEqual(self.run_trace(self.repo, 'summary'), f'built {head} two: summary')
        self.assertEqual(len(self.built()), 3, 'an edited tree builds on every run; cargo decides what is fresh')
        self.assertEqual(self.binaries(), [head])
        (self.repo / 'apps' / 'app.rs').write_text('one')
        self.assertEqual(self.run_trace(self.repo, 'summary'), f'built {head} one: summary')
        self.assertEqual(len(self.built()), 3, 'back at the commit, the cached binary runs')

    def test_each_commit_gets_its_own_binary_and_only_the_newest_five_are_kept(self):
        commits = [git(self.repo, 'rev-parse', 'HEAD')]
        self.run_trace(self.repo, 'summary')
        for n in range(6):
            (self.repo / 'apps' / 'app.rs').write_text(f'v{n}')
            commits.append(self.commit(f'v{n}'))
            self.assertEqual(self.run_trace(self.repo, 'summary'), f'built {commits[-1]} v{n}: summary')
        self.assertEqual(len(self.built()), 7)
        self.assertEqual(self.binaries(), sorted(commits[-5:]))

    def test_a_commit_made_while_it_builds_is_not_cached_under_either_commit(self):
        head = git(self.repo, 'rev-parse', 'HEAD')
        commit = 'git -C "$manifest" -c user.name=t -c user.email=t@t commit -q --allow-empty -m moved'
        out = self.run_trace(self.repo, 'summary', WHILE_BUILDING=commit)
        self.assertEqual(out, f'built {head} one: summary', 'runs what it built')
        self.assertEqual(self.binaries(), [])
        moved = git(self.repo, 'rev-parse', 'HEAD')
        self.assertEqual(self.run_trace(self.repo, 'summary'), f'built {moved} one: summary')
        self.assertEqual(self.binaries(), [moved])

    def test_an_edit_made_while_it_builds_is_not_cached(self):
        self.run_trace(self.repo, 'summary', WHILE_BUILDING='echo two > "$manifest/apps/app.rs"')
        self.assertEqual(self.binaries(), [])

    def test_an_untracked_build_input_counts_as_an_edit_and_an_ignored_one_does_not(self):
        (self.repo / '.gitignore').write_text('*.tmp\n')
        git(self.repo, 'add', '.gitignore')
        head = self.commit('ignore')
        self.run_trace(self.repo, 'summary')
        (self.repo / 'apps' / 'scratch.tmp').write_text('ignored')
        self.run_trace(self.repo, 'summary')
        self.assertEqual(len(self.built()), 1, 'an ignored file builds nothing')
        (self.repo / 'apps' / 'extra.rs').write_text('untracked')
        self.run_trace(self.repo, 'summary')
        self.assertEqual(len(self.built()), 2, 'an untracked input builds the working tree')
        self.assertEqual(self.binaries(), [head])

    def test_pruning_keeps_the_five_used_last_and_drops_stale_temporary_copies(self):
        first = git(self.repo, 'rev-parse', 'HEAD')
        self.run_trace(self.repo, 'summary')
        self.cache().joinpath(f'.{first}.Abc123').write_text('left by a killed install')
        os.utime(self.cache() / f'.{first}.Abc123', (0, 0))
        self.cache().joinpath(f'.{first}.Fresh1').write_text('an install in progress')
        commits = [first]
        for n in range(4):
            (self.repo / 'apps' / 'app.rs').write_text(f'v{n}')
            commits.append(self.commit(f'v{n}'))
            self.run_trace(self.repo, 'summary')
        worktree = self.tmp / 'worktree'
        git(self.repo, 'worktree', 'add', '-q', '--detach', str(worktree), first)
        self.run_trace(worktree, 'summary')
        (self.repo / 'apps' / 'app.rs').write_text('v4')
        commits.append(self.commit('v4'))
        self.run_trace(self.repo, 'summary')
        names = sorted(p.name for p in self.cache().iterdir())
        self.assertEqual(
            names, sorted([f'.{first}.Fresh1', '.lock', first, *commits[2:]]), 'the oldest unused one goes'
        )

    @contextmanager
    def locked(self, path, how):
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, 'w') as lock:
            fcntl.flock(lock, how)
            yield

    def waits_for(self, lock, *args):
        with self.locked(lock, fcntl.LOCK_EX):
            waiting = subprocess.Popen(
                ['bash', 'tools/simulator/trace.sh', 'trace', *args],
                cwd=self.repo,
                env=self.env,
                stdout=subprocess.PIPE,
                text=True,
            )
            time.sleep(0.5)
            self.assertIsNone(waiting.poll(), 'waits while the lock is held')
        out, _ = waiting.communicate(timeout=30)
        self.assertEqual(waiting.returncode, 0)
        return out.strip()

    def binaries(self):
        return sorted(p.name for p in self.cache().iterdir() if not p.name.startswith('.'))

    def test_a_binary_that_names_another_commit_runs_and_is_not_cached(self):
        head = git(self.repo, 'rev-parse', 'HEAD')
        self.assertEqual(self.run_trace(self.repo, 'summary', STAMP='another'), f'built {head} one: summary')
        self.assertEqual(self.binaries(), [])
        self.assertEqual(list(self.cache().glob('.*.*')), [], 'its copy is gone')

    def test_a_build_waits_for_one_in_the_same_target_dir(self):
        head = git(self.repo, 'rev-parse', 'HEAD')
        lock = self.tmp / 'target' / '.trace-build.lock'
        self.assertEqual(self.waits_for(lock, 'summary'), f'built {head} one: summary')
        self.assertEqual(len(self.built()), 1)

    def test_a_cached_binary_runs_holding_the_cache_lock(self):
        self.run_trace(self.repo, 'summary')
        out = self.run_trace(self.repo, 'summary', PROBE_LOCK=str(self.cache() / '.lock'))
        self.assertEqual(out.splitlines()[0], 'locked')

    def test_pruning_skips_while_a_cached_binary_runs(self):
        with self.locked(self.cache() / '.lock', fcntl.LOCK_SH):
            for n in range(6):
                (self.repo / 'apps' / 'app.rs').write_text(f'v{n}')
                self.commit(f'v{n}')
                self.run_trace(self.repo, 'summary')
            self.assertEqual(len(self.binaries()), 6)
        (self.repo / 'apps' / 'app.rs').write_text('v6')
        self.commit('v6')
        self.run_trace(self.repo, 'summary')
        self.assertEqual(len(self.binaries()), 5)

    def test_a_lookup_waits_while_pruning(self):
        head = git(self.repo, 'rev-parse', 'HEAD')
        self.run_trace(self.repo, 'summary')
        self.assertEqual(self.waits_for(self.cache() / '.lock', 'find'), f'built {head} one: find')
        self.assertEqual(len(self.built()), 1, 'the cached binary ran')


class RoutingTests(unittest.TestCase):
    """The argv `cycling trace` and `cycling replay` pass on, through the nix wrapper and the task runner."""

    def setUp(self):
        launcher = TraceLauncherTests('run')
        launcher.setUp()
        self.addCleanup(launcher.doCleanups)
        self.launcher = launcher
        self.repo = launcher.repo
        head = git(self.repo, 'rev-parse', 'HEAD')
        self.built = f'built {head} one:'
        shell = (ROOT / 'nix' / 'shell.nix').read_text()
        body = re.search(r'writeShellScriptBin "cycling" \'\'\n(.*?)\n\s*\'\';', shell, re.S).group(1)
        self.wrapper = self.launcher.tmp / 'cycling'
        self.wrapper.write_text(body.replace("''$", '$'))

    def cycling(self, *args, **env):
        return self.launcher.launch(['bash', str(self.wrapper), *args], **env)

    def task(self, name, *args, **env):
        task = cli.BY_NAME[name]
        return self.launcher.launch(cli.bash(task.steps[-1], task, list(args), last=True), **env)

    def test_queries_reach_the_tool_with_or_without_a_leading_double_dash(self):
        for args, said in (
            (('trace', 'summary', 'x.trace'), 'summary x.trace'),
            (('trace', '--', 'find', 'x.trace', 'mark'), 'find x.trace mark'),
            (('replay', 'x.trace', '--from', '1:00'), 'replay x.trace --from 1:00'),
            (('replay', '--', 'x.trace'), 'replay x.trace'),
        ):
            done = self.cycling(*args)
            self.assertEqual((done.returncode, done.stdout.strip()), (0, f'{self.built} {said}'), args)

    def test_the_task_runner_runs_the_same_launcher(self):
        for args, said in ((('trace', 'find', 'x.trace'), 'find x.trace'), (('replay', 'x.trace'), 'replay x.trace')):
            done = self.task(*args)
            self.assertEqual((done.returncode, done.stdout.strip()), (0, f'{self.built} {said}'), args)

    def test_pull_goes_to_the_device_tools(self):
        expected = (
            f'uv run --project {self.repo}/tools/devtools --locked python -m cycling_devtools.device.volumes trace-pull'
        )
        for done in (
            self.cycling('trace', 'pull', 'a.trace', '--json'),
            self.cycling('trace', '--', 'pull', 'a.trace', '--json'),
            self.task('trace', 'pull', 'a.trace', '--json'),
        ):
            self.assertEqual((done.returncode, done.stdout.strip()), (0, f'{expected} a.trace --json'))

    def test_the_exit_status_comes_through(self):
        self.assertEqual(self.cycling('trace', 'summary', 'x.trace', TRACE_EXIT='2').returncode, 2)
        self.assertEqual(self.task('replay', 'x.trace', TRACE_EXIT='3').returncode, 3)
        self.assertEqual(self.cycling('trace', 'pull', UV_EXIT='4').returncode, 4)
        refused = self.launcher.launch(['bash', 'tools/simulator/trace.sh', 'summary', 'x.trace'])
        self.assertEqual(refused.returncode, 2, 'the launcher names its task first')

    def test_other_tasks_go_to_the_task_runner(self):
        done = self.cycling('sim', '--', 'x')
        self.assertEqual(
            done.stdout.strip(),
            f'uv run --project {self.repo}/tools/devtools --locked python -m cycling_devtools.cli sim -- x',
        )


if __name__ == '__main__':
    unittest.main()
