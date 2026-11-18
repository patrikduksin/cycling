import hashlib
import json
import os
import subprocess
import time

import pytest

from cycling_devtools import twin


def git(cwd, *args):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def commit(repo, message):
    git(repo, 'add', '-A')
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '-m', message)
    return git(repo, 'rev-parse', 'HEAD')


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / 'repo'
    root.mkdir()
    (root / 'version').write_text('one\n')
    (root / '.gitignore').write_text('/.local/\n/target/\n')
    git(root, 'init', '-q', '-b', 'main')
    commit(root, 'first')
    return root


class Builder:
    def __init__(self, fail=False):
        self.built = []
        self.fail = fail

    def __call__(self, source, into):
        self.built.append(source)
        if self.fail:
            raise subprocess.CalledProcessError(101, 'cargo')
        version = (source / 'version').read_text()
        out = source / 'target' / 'trace'
        out.parent.mkdir(parents=True, exist_ok=True)
        name = hashlib.sha256(version.encode()).hexdigest()[:40]
        out.write_text(f'#!/bin/sh\necho {name}\n')
        out.chmod(0o755)
        return twin.private_copy(out, into)


def worktrees(repo):
    return [line for line in git(repo, 'worktree', 'list', '--porcelain').splitlines() if line.startswith('worktree ')]


def test_a_clean_commit_is_cached_by_its_id_and_built_once(repo):
    builder = Builder()
    head = git(repo, 'rev-parse', 'HEAD')
    built = twin.binary('HEAD', root=repo, builder=builder)
    assert built == repo / '.local' / 'tools' / 'trace' / head
    assert twin.binary(head, root=repo, builder=builder) == built
    assert len(builder.built) == 1
    assert len(worktrees(repo)) == 1, 'the scratch worktree is gone'
    assert not builder.built[0].exists()


def test_a_patched_build_is_cached_by_its_own_hash_and_never_served_as_the_commit(repo):
    builder = Builder()
    head = git(repo, 'rev-parse', 'HEAD')
    (repo / 'version').write_text('two\n')
    patch = subprocess.run(['git', 'diff', '--binary'], cwd=repo, check=True, capture_output=True).stdout
    git(repo, 'checkout', '--', 'version')
    patched = twin.binary(head, patch, root=repo, builder=builder)
    own = hashlib.sha256(b'two\n').hexdigest()[:40]
    assert patched.name == f'build-{own}'
    assert twin.build_hash_of(patched) == own
    assert twin.binary(head, patch, root=repo, builder=builder) == patched, 'the same patch is cached'
    clean = twin.binary(head, root=repo, builder=builder)
    assert clean.name == head
    assert twin.build_hash_of(clean) == hashlib.sha256(b'one\n').hexdigest()[:40]
    assert len(builder.built) == 2


def test_the_scratch_worktree_goes_when_the_build_fails(repo):
    with pytest.raises(subprocess.CalledProcessError):
        twin.binary('HEAD', root=repo, builder=Builder(fail=True))
    assert len(worktrees(repo)) == 1
    assert not any(path.name.startswith('twin-') for path in (repo / '.local' / 'tools').iterdir())


def test_a_tree_build_is_keyed_by_its_tree_with_uncommitted_edits(repo):
    builder = Builder()
    first = twin.binary_for_tree(repo, root=repo, builder=builder)
    assert twin.binary_for_tree(repo, root=repo, builder=builder) == first
    (repo / 'version').write_text('edited\n')
    edited = twin.binary_for_tree(repo, root=repo, builder=builder)
    assert edited != first
    assert twin.build_hash_of(edited) == hashlib.sha256(b'edited\n').hexdigest()[:40]
    assert len(builder.built) == 2


def test_an_archived_build_rebuilds_from_its_commit_and_patch(repo):
    builder = Builder()
    head = git(repo, 'rev-parse', 'HEAD')
    archived = repo / '.local' / 'builds' / ('ab' * 20)
    archived.mkdir(parents=True)
    (archived / 'commit').write_text(f'{head}\n')
    built = twin.binary_for_build('ab' * 20, root=repo, builder=builder)
    assert built.name == head
    (repo / 'version').write_text('three\n')
    (archived / 'diff.patch').write_bytes(
        subprocess.run(['git', 'diff', '--binary'], cwd=repo, check=True, capture_output=True).stdout
    )
    git(repo, 'checkout', '--', 'version')
    patched = twin.binary_for_build('ab' * 20, root=repo, builder=builder)
    assert twin.build_hash_of(patched) == hashlib.sha256(b'three\n').hexdigest()[:40]
    with pytest.raises(twin.NotArchived):
        twin.binary_for_build('cd' * 20, root=repo, builder=builder)


def test_a_twin_gives_a_faithful_verdict_only_as_the_recording_build_or_its_sources(repo, monkeypatch):
    builder = Builder()
    built = twin.binary('HEAD', root=repo, builder=builder)
    own = twin.build_hash_of(built)
    assert twin.faithful_to(built, own, root=repo) is None
    monkeypatch.setattr(twin.build_hash, 'relate', lambda builds, a, b: 'same-sources')
    assert twin.faithful_to(built, 'ef' * 20, root=repo) is None
    monkeypatch.setattr(twin.build_hash, 'relate', lambda builds, a, b: 'other')
    assert twin.faithful_to(built, 'ef' * 20, root=repo) == (
        f'the twin {own[:12]} is not the build efefefefefef that recorded the trace: other'
    )


def test_the_built_binary_is_copied_out_under_the_lock_before_another_build_can_replace_it(tmp_path, monkeypatch):
    target, tools = tmp_path / 'target', tmp_path / 'bin'
    executable = target / 'tools' / 'trace'
    tools.mkdir()
    built = json.dumps({'reason': 'compiler-artifact', 'target': {'name': 'trace'}, 'executable': str(executable)})
    (tools / 'cargo').write_text(
        '#!/bin/sh\n'
        'case "$1" in\n'
        f"  metadata) echo '{json.dumps({'target_directory': str(target)})}' ;;\n"
        f'  build) mkdir -p {executable.parent} && printf "#!/bin/sh\\necho first\\n" > {executable}'
        f" && chmod +x {executable} && echo '{built}' ;;\n"
        'esac\n'
    )
    (tools / 'cargo').chmod(0o755)
    monkeypatch.setenv('PATH', f'{tools}:{os.environ["PATH"]}')
    into = tmp_path / 'cache'
    copy = twin.build(tmp_path, into)
    executable.write_text('#!/bin/sh\necho second\n')
    assert copy.parent == into
    assert twin.build_hash_of(copy) == 'first', 'a later build in the shared target dir leaves the copy alone'
    assert not os.access(copy, os.W_OK)


def test_a_build_whose_hash_cannot_be_read_leaves_no_private_copy(repo):
    def builder(source, into):
        copy = Builder()(source, into)
        copy.chmod(0o755)
        copy.write_text('#!/bin/sh\nexit 1\n')
        return copy

    (repo / 'version').write_text('two\n')
    patch = subprocess.run(['git', 'diff', '--binary'], cwd=repo, check=True, capture_output=True).stdout
    git(repo, 'checkout', '--', 'version')
    with pytest.raises(subprocess.CalledProcessError):
        twin.binary('HEAD', patch, root=repo, builder=builder)
    assert list((repo / '.local' / 'tools' / 'trace').iterdir()) == []


def test_a_build_whose_sources_lack_a_mode_is_refused_before_anything_is_built(repo):
    builder = Builder()
    old = git(repo, 'rev-parse', 'HEAD')
    with pytest.raises(twin.Incapable, match=f'the trace tool of {old[:12]} has no --mode faithful'):
        twin.binary(old, root=repo, builder=builder, needs='faithful')
    listed = repo / 'tools' / 'simulator' / 'trace-capabilities'
    listed.parent.mkdir(parents=True)
    listed.write_text('faithful\nchanged\n')
    git(repo, 'add', '-N', '.')
    adds = subprocess.run(['git', 'diff', '--binary'], cwd=repo, check=True, capture_output=True).stdout
    assert twin.binary(old, adds, root=repo, builder=builder, needs='faithful').is_file(), 'a patch can add it'
    new = commit(repo, 'modes')
    assert twin.binary(new, root=repo, builder=builder, needs='faithful') == repo / '.local' / 'tools' / 'trace' / new
    (repo / 'version').write_text('two\n')
    other = subprocess.run(['git', 'diff', '--binary'], cwd=repo, check=True, capture_output=True).stdout
    git(repo, 'checkout', '--', 'version')
    with pytest.raises(twin.Incapable):
        twin.binary(old, other, root=repo, builder=builder, needs='faithful')
    assert len(builder.built) == 2


def test_the_pull_check_skips_a_build_older_than_the_faithful_mode_without_building_it(repo):
    from cycling_devtools.device import trace_media

    head = git(repo, 'rev-parse', 'HEAD')
    archived = repo / '.local' / 'builds' / ('ab' * 20)
    archived.mkdir(parents=True)
    (archived / 'commit').write_text(f'{head}\n')
    trace = repo / 'ride.trace'
    trace.write_bytes(b'a ride')
    checked = trace_media.faithful(trace, 'ab' * 20, root=repo)
    assert checked == {'skipped': f'the trace tool of {head[:12]} has no --mode faithful'}
    assert not (repo / '.local' / 'tools').exists(), 'nothing was built'


def test_against_names_where_the_two_replays_frames_part(repo, tmp_path, monkeypatch, capsys):
    tool = tmp_path / 'trace'
    panel = {'a': 'frame 000000ab', 'b': 'frame 000000cd', 'at_ms': 2000}
    diffed = {'seen': None, 'panel': panel, 'world': None, 'exclusions': ['2 phone-page frames of a: redacted']}
    tool.write_text(
        '#!/bin/sh\n'
        f"""if [ "$1" = diff ]; then echo '{json.dumps(diffed)}'; exit 0; fi\n"""
        'for arg; do [ "$prev" = --out ] && out=$arg; prev=$arg; done\n'
        'mkdir -p "$out" && : > "$out/replayed.trace"\n'
        """echo '{"divergence": {"ok": true, "verdict": "holds"}}'\n"""
    )
    tool.chmod(0o755)
    monkeypatch.setattr(twin, 'binary', lambda *args, **kwargs: tool)
    monkeypatch.setattr(twin, 'binary_for_tree', lambda *args, **kwargs: tool)
    trace = tmp_path / 'ride.trace'
    trace.write_bytes(b'a ride')
    assert twin.against([str(trace), '--against', 'HEAD', '--out', str(tmp_path / 'out')], root=repo) == 0
    printed = capsys.readouterr().out
    assert 'panel: frame 000000ab | frame 000000cd at 2000 ms' in printed
    assert 'left out: 2 phone-page frames of a: redacted' in printed


def test_the_private_copy_is_new_so_the_cache_prune_never_takes_it_for_a_stale_one(tmp_path):
    binary = tmp_path / 'trace'
    binary.write_text('#!/bin/sh\n')
    old = time.time() - 2 * 60 * 60
    os.utime(binary, (old, old))
    copy = twin.private_copy(binary, tmp_path / 'cache')
    assert time.time() - copy.stat().st_mtime < 60, 'trace.sh prunes copies older than an hour'
