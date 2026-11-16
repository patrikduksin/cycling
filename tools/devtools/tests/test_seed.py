"""Seeding a fresh worktree's target dirs: which checkout it copies, what it copies, and when."""

import fcntl
import os
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from cycling_devtools import cli, seed

OLD = 1_700_000_000


def checkout(path, committed):
    path.mkdir(parents=True, exist_ok=True)
    subprocess.run(['git', 'init', '-q', str(path)], check=True)
    stamp = f'@{committed} +0000'
    env = os.environ | {'GIT_COMMITTER_DATE': stamp, 'GIT_AUTHOR_DATE': stamp}
    subprocess.run(
        ['git', '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '--allow-empty', '-m', 'x'],
        cwd=path,
        env=env,
        check=True,
    )
    return path


def built(path, *relatives):
    for relative in relatives:
        (path / relative / 'debug').mkdir(parents=True, exist_ok=True)
        (path / relative / 'debug' / 'lib.rlib').write_text(f'{relative} of {path.name}')
        os.utime(path / relative / 'debug' / 'lib.rlib', (OLD, OLD))
    return path


class Case(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve()


class SourceTests(Case):
    def setUp(self):
        super().setUp()
        self.main = checkout(self.base / 'main', OLD + 100)
        self.merge = self.main / '.local/merge/verify'

    def test_the_merge_checkout_on_a_newer_commit_wins(self):
        built(self.main, 'target')
        built(checkout(self.merge, OLD + 200), 'target')
        self.assertEqual(seed.source(self.main), self.merge)

    def test_the_main_checkout_wins_when_the_merge_checkout_is_older(self):
        built(self.main, 'target')
        built(checkout(self.merge, OLD), 'target')
        self.assertEqual(seed.source(self.main), self.main)

    def test_the_main_checkout_without_a_merge_checkout(self):
        built(self.main, '.local/firmware')
        self.assertEqual(seed.source(self.main), self.main)

    def test_a_checkout_that_never_built_is_no_source(self):
        checkout(self.merge, OLD + 200)
        self.assertIsNone(seed.source(self.main))
        built(self.main, 'target')
        self.assertEqual(seed.source(self.main), self.main)


class SeedTests(Case):
    def setUp(self):
        super().setUp()
        self.warm = built(self.base / 'warm', 'target', '.local/firmware')
        self.fresh = self.base / 'fresh'
        self.fresh.mkdir()

    def test_copies_only_the_missing_dirs_with_their_mtimes(self):
        (self.fresh / '.local/firmware').mkdir(parents=True)
        (self.fresh / '.local/firmware/mine').write_text('own build')
        self.assertEqual(seed.seed(self.fresh, self.warm), ['target'])
        copied = self.fresh / 'target/debug/lib.rlib'
        self.assertEqual(copied.read_text(), 'target of warm')
        self.assertEqual(copied.stat().st_mtime, OLD)
        self.assertEqual(os.listdir(self.fresh / '.local/firmware'), ['mine'])
        self.assertFalse((self.fresh / 'vendor').exists())

    def test_a_second_run_and_a_crashed_one_converge(self):
        (self.fresh / seed.STAGING / 'target/debug').mkdir(parents=True)
        (self.fresh / seed.STAGING / 'target/debug/half').write_text('torn')
        self.assertEqual(seed.seed(self.fresh, self.warm), ['target', '.local/firmware'])
        self.assertEqual(seed.seed(self.fresh, self.warm), [])
        self.assertFalse((self.fresh / 'target/debug/half').exists())

    def test_a_checkout_never_seeds_from_itself(self):
        self.assertEqual(seed.seed(self.warm, self.warm), [])

    def test_waits_for_a_running_build_before_copying(self):
        lock = self.warm / 'target/debug/.cargo-lock'
        lock.touch()
        copied = threading.Event()

        def run():
            seed.seed(self.fresh, self.warm)
            copied.set()

        with lock.open('a') as build:
            fcntl.flock(build, fcntl.LOCK_SH)
            with patch.object(seed.time, 'sleep', lambda _: copied.wait(0.01)):
                thread = threading.Thread(target=run)
                thread.start()
                self.assertFalse(copied.wait(0.3))
                self.assertFalse((self.fresh / 'target').exists())
        thread.join(5)
        self.assertTrue(copied.is_set())
        self.assertTrue((self.fresh / 'target/debug/lib.rlib').exists())


class CliTests(Case):
    def run_task(self, builds):
        warm = built(self.base / 'warm', 'target')
        fresh = self.base / f'fresh-{builds}'
        fresh.mkdir()
        task = cli.Task('t', 'test', (f'test -d {fresh}/target',), builds=builds)
        with patch.object(seed, 'ROOT', fresh), patch.object(seed, 'source', return_value=warm):
            return cli.run(task)

    def test_a_task_that_builds_finds_its_target_dirs_seeded(self):
        self.assertEqual(self.run_task(builds=True), 0)

    def test_a_task_that_does_not_build_copies_nothing(self):
        self.assertNotEqual(self.run_task(builds=False), 0)

    def test_the_gates_build(self):
        for name in ('verify', 'test', 'check', 'build', 'sim-compare'):
            self.assertTrue(cli.BY_NAME[name].builds, name)


if __name__ == '__main__':
    unittest.main()
