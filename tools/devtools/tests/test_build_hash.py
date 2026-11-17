import hashlib
import os
import subprocess
import time

import pytest

from cycling_devtools import build_hash


def git(cwd, *args):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def commit(repo, message):
    git(repo, 'add', '-A')
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '-m', message)


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / 'repo'
    (root / 'apps').mkdir(parents=True)
    (root / 'tools' / 'simulator').mkdir(parents=True)
    (root / 'tools' / 'simulator' / 'build-sources').write_text('apps\nCargo.lock\n')
    (root / 'apps' / 'app.rs').write_text('fn main() {}\n')
    (root / 'Cargo.lock').write_text('version = 4\n')
    (root / 'notes.md').write_text('notes\n')
    (root / '.gitignore').write_text('/.local/\n*.tmp\n')
    git(root, 'init', '-q', '-b', 'main')
    commit(root, 'first')
    return root


@pytest.fixture
def rustc(tmp_path):
    path = tmp_path / 'rustc'
    path.write_text('#!/bin/sh\necho "rustc ${RUSTC_RELEASE:-1.100.0}"\n')
    path.chmod(0o755)
    return path


@pytest.fixture
def build(repo, rustc, capsys):
    def built(root=repo, scope='tree', features='', vana=''):
        argv = ['--root', str(root), 'hash', '--scope', scope, '--rustc', str(rustc)]
        assert build_hash.main([*argv, '--features', features, '--vana-features', vana]) == 0
        name, state = capsys.readouterr().out.split()
        return name, state

    return built


@pytest.fixture
def cli(repo, capsys):
    def run(*args):
        status = build_hash.main(['--root', str(repo), *args])
        return status, capsys.readouterr().out.strip()

    return run


def test_the_hash_is_twenty_bytes_of_its_archived_listing(repo, build):
    name, state = build(features='testing', vana='invariants')
    archived = repo / '.local' / 'builds' / name
    listing = (archived / 'listing').read_text()
    assert len(bytes.fromhex(name)) == build_hash.HEADER_BUILD_BYTES
    assert hashlib.sha256(listing.encode()).hexdigest()[:40] == name
    assert listing.splitlines() == [
        f'tree {git(repo, "rev-parse", "HEAD^{tree}")}',
        f'source apps {git(repo, "rev-parse", "HEAD:apps")}',
        f'source Cargo.lock {git(repo, "rev-parse", "HEAD:Cargo.lock")}',
        'features testing',
        'vana-features invariants',
        'toolchain rustc 1.100.0',
    ]
    assert (archived / 'commit').read_text().strip() == git(repo, 'rev-parse', 'HEAD')
    assert state == 'clean'
    assert not (archived / 'diff.patch').exists()


def test_identical_trees_name_the_same_build_and_an_ignored_file_changes_nothing(repo, build, tmp_path):
    first = build()
    other = tmp_path / 'other'
    git(repo, 'worktree', 'add', '-q', '--detach', str(other))
    assert build(other) == first
    (repo / 'apps' / 'scratch.tmp').write_text('ignored')
    assert build() == first


@pytest.mark.parametrize(
    'change',
    [
        lambda repo, env: (repo / 'apps' / 'new.rs').write_text('untracked\n'),
        lambda repo, env: (repo / 'apps' / 'app.rs').write_text('fn main() { edited }\n'),
        lambda repo, env: (repo / 'Cargo.lock').write_text('version = 5\n'),
        lambda repo, env: (repo / 'notes.md').write_text('edited\n'),
        lambda repo, env: env.setenv('RUSTC_RELEASE', '1.101.0'),
    ],
    ids=['untracked file', 'tracked edit', 'Cargo.lock', 'outside the sources', 'toolchain'],
)
def test_each_input_moves_the_firmware_hash(repo, build, monkeypatch, change):
    before, _ = build()
    change(repo, monkeypatch)
    assert build()[0] != before


def test_the_simulator_hash_moves_with_its_sources_only(repo, build):
    before, _ = build(scope='sources')
    (repo / 'notes.md').write_text('edited\n')
    assert build(scope='sources') == (before, 'dirty')
    (repo / 'apps' / 'added.rs').write_text('new\n')
    assert build(scope='sources')[0] != before


def test_features_move_the_hash_however_cargo_spells_them(build):
    none, _ = build()
    gnss, _ = build(features='scripted-gnss,bulk')
    assert gnss != none
    assert build(features='BULK,scripted_gnss')[0] == gnss
    assert build(vana='planted_bug')[0] not in (none, gnss)


def test_a_dirty_build_archives_the_patch_that_rebuilds_its_tree(repo, build, tmp_path):
    (repo / 'apps' / 'app.rs').write_text('fn main() { edited }\n')
    (repo / 'apps' / 'new.rs').write_text('untracked\n')
    name, state = build()
    assert state == 'dirty'
    archived = repo / '.local' / 'builds' / name
    rebuilt = tmp_path / 'rebuilt'
    git(repo, 'worktree', 'add', '-q', '--detach', str(rebuilt), (archived / 'commit').read_text().strip())
    git(rebuilt, 'apply', '--binary', str(archived / 'diff.patch'))
    assert (rebuilt / 'apps' / 'new.rs').read_text() == 'untracked\n'
    assert build(rebuilt) == (name, 'dirty')


def test_dirty_is_this_tree_against_head_whatever_the_archive_holds(repo, build):
    first = git(repo, 'rev-parse', 'HEAD')
    (repo / 'apps' / 'app.rs').write_text('fn main() { edited }\n')
    name, state = build()
    assert state == 'dirty'
    commit(repo, 'edited')
    assert build() == (name, 'clean'), 'committed since'
    git(repo, 'reset', '-q', '--soft', first)
    assert build() == (name, 'dirty'), 'the archive of the clean build has no patch'


def test_relate_names_the_firmware_and_simulator_builds_of_one_tree_same_sources(build, cli):
    firmware, _ = build(features='scripted-gnss')
    simulator, _ = build(scope='sources', features='live,default')
    assert cli('relate', firmware, firmware) == (0, 'same')
    assert cli('relate', firmware, simulator) == (0, 'same-sources')
    assert cli('relate', simulator, firmware) == (0, 'same-sources')


def test_relate_tells_other_sources_and_a_planted_bug_apart(repo, build, cli):
    clean, _ = build(scope='sources')
    planted, _ = build(scope='sources', vana='planted-bug')
    assert cli('relate', clean, planted) == (0, 'other')
    (repo / 'apps' / 'app.rs').write_text('fn main() { edited }\n')
    edited, _ = build(scope='sources')
    assert cli('relate', clean, edited) == (0, 'other')


def test_relate_is_unknown_without_both_archived_listings(repo, build, cli):
    built, _ = build()
    missing = 'f' * 40
    assert cli('relate', built, missing) == (0, f'unknown build {missing[:12]} has no archived sources on this machine')
    old = repo / '.local' / 'builds' / ('e' * 40)
    old.mkdir()
    (old / 'listing').write_text('tree 0123\nfeatures \n')
    assert cli('relate', built, old.name)[1].startswith('unknown build eeeeeeeeeeee')


def test_built_from_holds_only_for_a_build_of_the_revisions_sources(repo, build, cli):
    clean, _ = build(scope='sources')
    assert cli('built-from', clean, 'HEAD')[0] == 0
    (repo / 'notes.md').write_text('edited\n')
    assert cli('built-from', build(scope='sources')[0], 'HEAD')[0] == 0, 'outside the sources'
    (repo / 'apps' / 'app.rs').write_text('fn main() { edited }\n')
    edited, _ = build(scope='sources')
    assert cli('built-from', edited, 'HEAD')[0] == 1
    commit(repo, 'edited')
    assert cli('built-from', edited, 'HEAD')[0] == 0
    assert cli('built-from', 'f' * 40, 'HEAD')[0] == 1


def test_listing_prints_what_a_build_archived(repo, build, cli):
    name, _ = build()
    assert cli('listing', name) == (0, (repo / '.local' / 'builds' / name / 'listing').read_text().strip())
    assert cli('listing', 'f' * 40)[0] == 1


def test_archiving_keeps_every_build(repo, build):
    builds = repo / '.local' / 'builds'
    first = build(features='a')[0]
    long_ago = time.time() - 365 * 86400
    os.utime(builds / first, (long_ago, long_ago))
    second = build(features='b')[0]
    assert sorted(p.name for p in builds.iterdir()) == sorted([first, second])
