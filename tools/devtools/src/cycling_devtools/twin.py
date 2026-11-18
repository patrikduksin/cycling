"""The trace tool, VANA's host twin, of any build: a commit, a commit and a patch, or a source tree.

A build runs as tools/simulator/trace.sh builds the tool, in a scratch detached worktree that is removed
afterwards, and the binary is cached in the primary checkout's .local/tools/trace, which trace.sh shares. A
clean commit is cached by its id, as trace.sh caches it. A patched or tree build is cached by the binary's own
build hash, beside a note of what it was built from, so it is never served as the clean commit.

A caller that needs a replay mode, or `dump`, names it, and a build whose sources do not list it in
tools/simulator/trace-capabilities is refused before anything is built.

`against` is `cycling replay <trace> --against <rev>`: both builds replay the trace changed with --model, and
`trace diff` names where the two replays part.
"""

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NamedTuple

from cycling_devtools import build_hash, tree, workspace

CAPABILITIES = 'tools/simulator/trace-capabilities'


class NotArchived(Exception):
    """The build hash names no archived build on this machine."""


class Incapable(Exception):
    """The trace tool of a build has no replay mode a caller needs."""


def cache(root=workspace.ROOT):
    return workspace.main_checkout(root) / '.local' / 'tools' / 'trace'


class Tool(NamedTuple):
    name: str = 'trace'
    profile: str = 'tools'
    features: tuple[str, ...] = ()


TRACE = Tool()


def build(source, into, tool=TRACE, *, cargo_target_dir=None, nix_develop_flake=None, runner=subprocess.run):
    if cargo_target_dir is None:
        metadata = runner(
            ['cargo', 'metadata', '--format-version', '1', '--no-deps', '--manifest-path', str(source / 'Cargo.toml')],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        cargo_target_dir = Path(json.loads(metadata)['target_directory'])
    cargo = [
        'cargo', 'build', '-q', '--manifest-path', str(source / 'Cargo.toml'), '--profile', tool.profile,
        '-p', 'shell-simulator', '--locked', '--no-default-features', '--bin', tool.name,
        '--message-format=json-render-diagnostics',
    ]  # fmt: skip
    if tool.features:
        cargo += ['--features', ','.join(tool.features)]
    argv = cargo if nix_develop_flake is None else ['nix', 'develop', str(nix_develop_flake), '--command', *cargo]
    cargo_target_dir = Path(cargo_target_dir)
    cargo_target_dir.mkdir(parents=True, exist_ok=True)
    with open(cargo_target_dir / '.trace-build.lock', 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        env = os.environ | {'CARGO_TARGET_DIR': str(cargo_target_dir)}
        done = runner(argv, cwd=source, env=env, check=True, stdout=subprocess.PIPE, text=True).stdout
        for line in done.splitlines():
            message = json.loads(line)
            built = message.get('reason') == 'compiler-artifact' and message['target']['name'] == tool.name
            if built and message.get('executable'):
                return private_copy(Path(message['executable']), into)
    raise RuntimeError(f'cargo built no {tool.name} binary in {source}')


def private_copy(binary, into):
    """`binary` copied to a new read-only file in `into`, which nothing else names."""
    into.mkdir(parents=True, exist_ok=True)
    handle, copy = tempfile.mkstemp(dir=into, prefix='.trace.')
    os.close(handle)
    copy = Path(copy)
    shutil.copy(binary, copy)
    copy.chmod(0o555)
    return copy


def build_hash_of(binary):
    return subprocess.run([str(binary), '--build-hash'], check=True, capture_output=True, text=True).stdout.strip()


@contextlib.contextmanager
def scratch_worktree(root, commit):
    """A detached worktree of `commit`, removed when the block ends, however it ends."""
    cache(root).parent.mkdir(parents=True, exist_ok=True)
    place = Path(tempfile.mkdtemp(prefix='twin-', dir=cache(root).parent))
    try:
        tree.git('worktree', 'add', '--detach', '--quiet', str(place), commit, cwd=root)
        yield place
    finally:
        subprocess.run(['git', 'worktree', 'remove', '--force', str(place)], cwd=root, capture_output=True)
        shutil.rmtree(place, ignore_errors=True)
        subprocess.run(['git', 'worktree', 'prune'], cwd=root, capture_output=True)


def from_note(key):
    return f'{key}\n'


def cached_by(directory, key):
    """The binary a patched or tree build of `key` left, when one did."""
    for note in sorted(directory.glob('build-*.from')):
        binary = note.with_suffix('')
        if note.read_text() == from_note(key) and binary.is_file():
            return binary
    return None


def keep(built, directory, name, key=None):
    """Move `built`, a private copy in the cache `directory`, to `name`, and note what it was built from."""
    final = directory / name
    built.replace(final)
    if key is not None:
        final.with_name(f'{name}.from').write_text(from_note(key))
    return final


@contextlib.contextmanager
def built_into(directory, builder, source):
    """The private copy `builder` made of the trace tool of `source`, removed unless the block kept it."""
    built = builder(source, directory)
    try:
        yield built
    finally:
        built.unlink(missing_ok=True)


def capabilities(commit, patch=None, *, root=workspace.ROOT):
    """The replay modes the trace tool of `commit`, with `patch`, has, read from its sources without a build."""
    with tempfile.TemporaryDirectory(prefix='capabilities-') as scratch:
        listed = Path(scratch) / CAPABILITIES
        listed.parent.mkdir(parents=True)
        shown = subprocess.run(['git', 'show', f'{commit}:{CAPABILITIES}'], cwd=root, capture_output=True)
        if shown.returncode == 0:
            listed.write_bytes(shown.stdout)
        if patch:
            subprocess.run(
                ['git', 'apply', '--binary', '--allow-empty', f'--include={CAPABILITIES}', '-'],
                cwd=scratch,
                input=patch,
                check=True,
                capture_output=True,
                env={**os.environ, 'GIT_CEILING_DIRECTORIES': str(Path(scratch).parent)},
            )
        return set(listed.read_text().split()) if listed.exists() else set()


def patched_key(commit, patch):
    return hashlib.sha256(commit.encode() + b'\0' + patch).hexdigest()


def binary(commit, patch=None, *, root=workspace.ROOT, builder=build, needs=None):
    """The trace tool of `commit`, with `patch` (bytes for `git apply --binary`) applied when given. With `needs`,
    a replay mode, a build that lacks it raises Incapable."""
    commit = tree.git('rev-parse', '--verify', f'{commit}^{{commit}}', cwd=root)
    if needs is not None and needs not in capabilities(commit, patch, root=root):
        raise Incapable(f'the trace tool of {commit[:12]} has no --mode {needs}')
    directory = cache(root)
    key = None if patch is None else patched_key(commit, patch)
    found = directory / commit if key is None else cached_by(directory, key)
    if found is not None and found.is_file():
        return found
    with scratch_worktree(root, commit) as source:
        if patch:
            subprocess.run(['git', 'apply', '--binary', '-'], cwd=source, input=patch, check=True)
        with built_into(directory, builder, source) as built:
            name = commit if key is None else f'build-{build_hash_of(built)}'
            return keep(built, directory, name, key)


def binary_for_tree(source, *, root=workspace.ROOT, builder=build):
    """The trace tool of the source tree at `source`, uncommitted edits included."""
    key = f'tree {tree.tree_hash(source)}'
    directory = cache(root)
    found = cached_by(directory, key)
    if found is not None:
        return found
    with built_into(directory, builder, Path(source)) as built:
        return keep(built, directory, f'build-{build_hash_of(built)}', key)


def binary_for_build(name, *, root=workspace.ROOT, builder=build, needs=None):
    """The trace tool of the archived build `name`: its commit, and its patch when it has one."""
    archived = build_hash.builds_dir(root) / name
    try:
        commit = (archived / 'commit').read_text().strip()
    except OSError:
        raise NotArchived(f'build {name[:12]} is not archived on this machine') from None
    patch_file = archived / 'diff.patch'
    patch = patch_file.read_bytes() if patch_file.exists() else None
    return binary(commit, patch, root=root, builder=builder, needs=needs)


def faithful_to(binary_path, recorded, *, root=workspace.ROOT):
    """Why `binary_path` cannot give a faithful verdict on a trace `recorded` built, or None when it can: it
    must be that build, or one of the same sources."""
    own = build_hash_of(binary_path)
    relation = build_hash.relate(build_hash.builds_dir(root), own, recorded)
    if relation in ('same', 'same-sources'):
        return None
    return f'the twin {own[:12]} is not the build {recorded[:12]} that recorded the trace: {relation}'


def run_json(binary_path, *args):
    done = subprocess.run([str(binary_path), *args, '--json'], capture_output=True, text=True)
    try:
        return json.loads(done.stdout)
    except json.JSONDecodeError:
        return {'error': done.stderr.strip() or f'{binary_path} printed no JSON'}


def against(args, *, root=workspace.ROOT):
    """`replay <trace> [options] --against REV`: replay under this tree and under REV, and diff them."""
    parser = argparse.ArgumentParser(prog='cycling replay --against', add_help=False)
    parser.add_argument('trace')
    parser.add_argument('--against', required=True)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--json', action='store_true')
    known, rest = parser.parse_known_args(args)
    trace = Path(known.trace).resolve()
    out = (known.out or Path('.local') / 'replay' / f'{trace.stem}-against').resolve()
    try:
        other = binary(known.against, root=root, needs='changed')
    except Incapable as error:
        if known.json:
            print(json.dumps({'error': str(error)}))
        else:
            print(error, file=sys.stderr)
        return 2
    this = binary_for_tree(root, root=root)
    replays = {}
    for name, built in (('this', this), ('against', other)):
        replays[name] = run_json(
            built, 'replay', str(trace), '--mode', 'changed', '--model', '--to', '99999:00', *rest,
            '--out', str(out / name),
        )  # fmt: skip
    traces = [out / name / 'replayed.trace' for name in replays]
    diff = run_json(this, 'diff', *map(str, traces)) if all(path.exists() for path in traces) else None
    result = {'against': known.against, 'out': str(out), **replays, 'diff': diff}
    if known.json:
        print(json.dumps(result))
    else:
        for name in replays:
            verdict = replays[name].get('divergence') or {}
            print(f'{name}: {replays[name].get("error") or verdict.get("verdict", "no verdict")}')
        if diff is None:
            print('diff: a replay wrote no trace')
        else:
            for part in ('seen', 'panel', 'world'):
                found = diff.get(part)
                text = 'the same' if found is None else f'{found["a"]} | {found["b"]} at {found["at_ms"]} ms'
                print(f'{part}: {text}')
            for exclusion in diff.get('exclusions', []):
                print(f'left out: {exclusion}')
        print(f'artifacts in {out}')
    return 0 if diff is not None and not any('error' in replay for replay in replays.values()) else 2


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    built = commands.add_parser('binary', help='print the path of the trace tool of a commit or an archived build')
    which = built.add_mutually_exclusive_group(required=True)
    which.add_argument('--commit')
    which.add_argument('--build')
    which.add_argument('--tree', type=Path)
    commands.add_parser('against', help='replay <trace> ... --against REV', add_help=False)
    args, rest = parser.parse_known_args(argv)
    match args.command:
        case 'binary':
            if args.commit:
                print(binary(args.commit))
            elif args.tree:
                print(binary_for_tree(args.tree))
            else:
                try:
                    print(binary_for_build(args.build))
                except NotArchived as error:
                    print(error, file=sys.stderr)
                    return 1
            return 0
        case 'against':
            return against(rest[1:] if rest[:1] == ['replay'] else rest)
    raise AssertionError(args.command)


if __name__ == '__main__':
    sys.exit(main())
