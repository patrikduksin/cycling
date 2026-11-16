"""tools/simulator/trace.sh: the trace tool of a clean commit is built once and shared by every worktree; an edited
tree builds and runs its own."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from cycling_devtools import cli
from cycling_devtools.workspace import ROOT

LAUNCHER = ROOT / 'tools' / 'simulator' / 'trace.sh'

# Stands in for cargo: it "builds" a trace tool that prints the commit and the tree it was built from, and logs
# each build.
FAKE_CARGO = """#!/usr/bin/env bash
set -eu
manifest=$(dirname "${4:?}")
mkdir -p "$CARGO_TARGET_DIR/tools"
tree=$(cat "$manifest/apps/app.rs")
commit=$(git -C "$manifest" rev-parse HEAD)
printf '#!/usr/bin/env bash\\necho "built %s %s: $*"\\n' "$commit" "$tree" > "$CARGO_TARGET_DIR/tools/trace"
chmod +x "$CARGO_TARGET_DIR/tools/trace"
echo "$manifest" >> "$BUILDS"
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
        (bin_dir / 'cargo').write_text(FAKE_CARGO)
        (bin_dir / 'cargo').chmod(0o755)
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

    def run_trace(self, checkout, *args):
        done = subprocess.run(
            ['bash', 'tools/simulator/trace.sh', *args],
            cwd=checkout,
            env=self.env,
            capture_output=True,
            text=True,
            check=True,
        )
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
        self.assertEqual(sorted(p.name for p in self.cache().iterdir()), [head])
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
        self.assertEqual(sorted(p.name for p in self.cache().iterdir()), sorted(commits[-5:]))


class RoutingTests(unittest.TestCase):
    def test_the_cycling_command_runs_trace_queries_with_the_launcher_the_task_runner_runs(self):
        launcher = cli.TRACE.removeprefix('bash ')
        self.assertTrue((ROOT / launcher).is_file())
        shell = (ROOT / 'nix' / 'shell.nix').read_text()
        self.assertEqual(shell.count(f'exec bash {launcher} '), 2, 'trace and replay in nix/shell.nix')


if __name__ == '__main__':
    unittest.main()
