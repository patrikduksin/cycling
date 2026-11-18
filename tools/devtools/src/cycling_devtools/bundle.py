"""A failure as one file any fleet machine replays: the trace, the TERRAIN pack it read, and the full source tree of
the build that recorded it.

  cycling bundle <trace> [--out FILE] [--json]    bundle a trace, expecting what a strict replay shows here
  cycling bundle run <file> [--json]              rebuild the recording build and replay or refuzz it
  python -m cycling_devtools.bundle write ...     the Rust tools' writer (replay and fuzz failures)
"""

import argparse
import contextlib
import fcntl
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import NamedTuple, assert_never

from cycling_devtools import build_hash, tree, workspace

FORMAT = 1
MANIFEST = 'bundle.json'
TRACE = 'trace'
ORIGIN = 'trace.origin'
COURSE = 'course.trace'
PACK = 'pack'
TREE = 'tree/'
COMMITTER = ('-c', 'user.name=cycling bundle', '-c', 'user.email=bundle@cycling.invalid', '-c', 'commit.gpgsign=false')
FINDING = re.compile(
    r'^FAIL seed=\S+ check=(?P<check>.*) at=(?P<at>setup|end|step \d+) t=\S+\n  (?P<detail>.*)$', re.MULTILINE
)

type Runner = Callable[..., subprocess.CompletedProcess]


class Refused(Exception):
    pass


@dataclass(frozen=True)
class PackId:
    sha256: str
    length: int


@dataclass(frozen=True)
class Replay:
    to_ms: int


@dataclass(frozen=True)
class Fuzz:
    seed: str
    course: bool
    debug_assertions: bool


type Run = Replay | Fuzz


@dataclass(frozen=True)
class Faithful:
    pass


@dataclass(frozen=True)
class Diverged:
    kind: str
    at_ms: int
    detail: str


@dataclass(frozen=True)
class Found:
    check: str
    at: str
    detail: str


@dataclass(frozen=True)
class NothingFound:
    pass


type Expect = Faithful | Diverged | Found
type Outcome = Expect | NothingFound


@dataclass(frozen=True)
class Manifest:
    format: int
    build: str
    listing: str
    pack: PackId | None
    run: Run
    expect: Expect


@dataclass(frozen=True)
class Reproduced:
    pass


@dataclass(frozen=True)
class Differs:
    expect: Expect
    got: Outcome


def verdict(expect: Expect, got: Outcome) -> Reproduced | Differs:
    return Reproduced() if got == expect else Differs(expect, got)


def is_count(value):
    return type(value) is int and value >= 0


def parse_run(raw) -> Run:
    match raw:
        case {'kind': 'replay', 'to_ms': to_ms} if is_count(to_ms):
            return Replay(to_ms)
        case {'kind': 'fuzz', 'seed': str(seed), 'course': bool(course), 'debug_assertions': bool(debug_assertions)}:
            return Fuzz(seed, course, debug_assertions)
    raise Refused(
        f'run is {{"kind": "replay", "to_ms": N}} or {{"kind": "fuzz", "seed": S, "course": B, "debug_assertions": B}}, '
        f'not {raw!r}'
    )


def parse_expect(raw) -> Expect:
    match raw:
        case {'faithful': True}:
            return Faithful()
        case {'diverged': {'kind': str(kind), 'at_ms': at_ms, 'detail': str(detail)}} if is_count(at_ms):
            return Diverged(kind, at_ms, detail)
        case {'found': {'check': str(check), 'at': str(at), 'detail': str(detail)}}:
            return Found(check, at, detail)
    raise Refused(f'expect is {{"faithful": true}}, {{"diverged": ...}} or {{"found": ...}}, not {raw!r}')


def parse_manifest(text) -> Manifest:
    try:
        raw = json.loads(text)
    except ValueError as error:
        raise Refused(f'{MANIFEST} is not JSON: {error}') from error
    if not isinstance(raw, dict) or raw.get('format') != FORMAT:
        found = raw.get('format') if isinstance(raw, dict) else None
        raise Refused(f'this bundle is format {found!r}; this checkout runs format {FORMAT}')
    match raw:
        case {'build': str(build), 'listing': str(listing), 'pack': pack, 'run': run, 'expect': expect}:
            pass
        case _:
            raise Refused(f'{MANIFEST} needs build, listing, pack, run and expect')
    if not re.fullmatch('[0-9a-f]{40}', build):
        raise Refused(f'{MANIFEST} names build {build!r}, not 40 hex digits')
    match pack:
        case None:
            pack_id = None
        case {'sha256': str(sha256), 'length': length} if is_count(length) and re.fullmatch('[0-9a-f]{64}', sha256):
            pack_id = PackId(sha256, length)
        case _:
            raise Refused(f'pack is null or {{"sha256": hex, "length": N}}, not {pack!r}')
    return Manifest(FORMAT, build, listing, pack_id, parse_run(run), parse_expect(expect))


def outcome_json(outcome: Outcome):
    match outcome:
        case Faithful():
            return {'faithful': True}
        case Diverged(kind, at_ms, detail):
            return {'diverged': {'kind': kind, 'at_ms': at_ms, 'detail': detail}}
        case Found(check, at, detail):
            return {'found': {'check': check, 'at': at, 'detail': detail}}
        case NothingFound():
            return {'passed': True}
        case _:
            assert_never(outcome)


def run_json(run: Run):
    match run:
        case Replay(to_ms):
            return {'kind': 'replay', 'to_ms': to_ms}
        case Fuzz(seed, course, debug_assertions):
            return {'kind': 'fuzz', 'seed': seed, 'course': course, 'debug_assertions': debug_assertions}
        case _:
            assert_never(run)


def manifest_json(manifest: Manifest):
    pack = manifest.pack and {'sha256': manifest.pack.sha256, 'length': manifest.pack.length}
    return {
        'format': manifest.format,
        'build': manifest.build,
        'listing': manifest.listing,
        'pack': pack,
        'run': run_json(manifest.run),
        'expect': outcome_json(manifest.expect),
    }


def bundles_dir(root):
    return workspace.main_checkout(root) / '.local' / 'bundles'


def file_sha256(path):
    with open(path, 'rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def staging_file(directory, prefix):
    descriptor, name = tempfile.mkstemp(dir=directory, prefix=prefix)
    os.close(descriptor)
    return Path(name)


class BuiltTree(NamedTuple):
    listing: str
    tree_id: str


def built_tree(root, build) -> BuiltTree:
    archived = build_hash.builds_dir(root) / build
    try:
        listing = (archived / 'listing').read_text()
        commit = (archived / 'commit').read_text().strip()
    except OSError:
        raise Refused(f'build {build} is not archived here; bundle it on the machine that built it') from None
    with tempfile.TemporaryDirectory() as scratch:
        env = {'GIT_INDEX_FILE': str(Path(scratch) / 'index')}
        tree.git('read-tree', commit, cwd=root, env=env)
        patch = archived / 'diff.patch'
        if patch.exists():
            tree.git('apply', '--cached', '--binary', str(patch), cwd=root, env=env)
        return BuiltTree(listing, tree.git('write-tree', cwd=root, env=env))


def add_tree(bundle, root, tree_id):
    """The tree's objects as they are: git archive would apply export-ignore and export-subst attributes."""
    listed = subprocess.run(['git', 'ls-tree', '-r', '-z', tree_id], cwd=root, check=True, capture_output=True).stdout
    with subprocess.Popen(
        ['git', 'cat-file', '--batch'], cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE
    ) as batch:
        for entry in filter(None, listed.split(b'\0')):
            meta, path = entry.split(b'\t', 1)
            mode, kind, object_id = meta.decode().split()
            if kind != 'blob':
                raise Refused(f'{os.fsdecode(path)} is a {kind}; a bundle holds files and symlinks only')
            batch.stdin.write(f'{object_id}\n'.encode())
            batch.stdin.flush()
            size = int(batch.stdout.readline().split()[2])
            info = tarfile.TarInfo(TREE + os.fsdecode(path))
            if mode == '120000':
                info.type, info.linkname = tarfile.SYMTYPE, os.fsdecode(batch.stdout.read(size))
                bundle.addfile(info)
            else:
                info.size, info.mode = size, 0o755 if mode == '100755' else 0o644
                bundle.addfile(info, batch.stdout)
            batch.stdout.read(1)
        batch.stdin.close()
    if batch.returncode:
        raise Refused(f'git cat-file of {tree_id} failed')


def write(root, out, trace, build, run: Run, expect: Expect, course=None, pack=None) -> Path:
    if isinstance(run, Fuzz) and run.course != (course is not None):
        raise Refused('a fuzz run rode a course exactly when --course names it')
    listing, tree_id = built_tree(root, build)
    pack_id = pack and PackId(file_sha256(pack), pack.stat().st_size)
    manifest = json.dumps(manifest_json(Manifest(FORMAT, build, listing, pack_id, run, expect)), indent=2).encode()
    origin = trace.with_suffix('.origin')
    members = [(trace, TRACE), (origin if origin.is_file() else None, ORIGIN), (course, COURSE), (pack, PACK)]
    out.parent.mkdir(parents=True, exist_ok=True)
    staging = staging_file(out.parent, f'.{out.name}.')
    try:
        with tarfile.open(staging, 'w:zst', dereference=True) as bundle:
            info = tarfile.TarInfo(MANIFEST)
            info.size, info.mtime, info.mode = len(manifest), int(time.time()), 0o644
            bundle.addfile(info, io.BytesIO(manifest))
            for path, name in members:
                if path is not None:
                    bundle.add(path, arcname=name)
            add_tree(bundle, root, tree_id)
        staging.chmod(0o644)
        os.replace(staging, out)
    finally:
        staging.unlink(missing_ok=True)
    return out


def trace_header(root, trace, runner: Runner):
    done = runner(
        ['bash', str(root / 'tools' / 'simulator' / 'trace.sh'), 'trace', 'summary', str(trace), '--json'],
        cwd=root,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        doc = json.loads(done.stdout)
    except ValueError:
        raise Refused(f'trace summary of {trace} printed no JSON') from None
    if done.returncode or 'error' in doc:
        raise Refused(f'trace summary of {trace}: {doc.get("error", "failed")}')
    header = doc['header']
    if 'terrain' not in header:
        raise Refused('this checkout\'s trace summary names no "terrain"; it predates bundles')
    return header


def find_pack(store, terrain) -> Path:
    length, sha256 = terrain['length'], terrain['sha256']
    candidates = sorted(path for path in store.glob('*') if path.is_file() and path.stat().st_size == length)
    for path in candidates:
        if file_sha256(path) == sha256:
            return path
    raise Refused(
        f'the session read the TERRAIN pack sha256 {sha256} ({length} bytes), and no file in {store} is that pack; '
        'put the pack file there, or name its directory in CYCLING_TERRAIN_PACKS'
    )


def bundle_trace(root, trace, out, runner: Runner) -> Path:
    header = trace_header(root, trace, runner)
    build = header['build']
    if build is None:
        raise Refused(f'{trace} names no build')
    terrain = header['terrain']
    pack = None
    if terrain is not None and terrain['ready']:
        store = os.environ.get('CYCLING_TERRAIN_PACKS') or workspace.main_checkout(root) / '.local' / 'terrain'
        pack = find_pack(Path(store), terrain)
    end_ms = header['end_ms']
    with tempfile.TemporaryDirectory() as scratch:
        tool = ['bash', str(root / 'tools' / 'simulator' / 'trace.sh')]
        doc = replay(tool, trace, end_ms, Path(scratch) / 'replay', root, None, runner)
    if doc['mode'] != 'strict':
        raise Refused(
            f'this checkout replays {trace.name} in {doc["mode"]} mode ({doc["why"]}), so it cannot say what a strict '
            "replay shows; bundle it from a checkout whose build matches the trace's"
        )
    out = out or bundles_dir(root) / f'{trace.stem}-{build[:12]}.bundle'
    return write(root, out, trace, build, Replay(end_ms), parted(doc), pack=pack)


@dataclass(frozen=True)
class Ran:
    manifest: Manifest
    verdict: Reproduced | Differs
    rebuilt_s: float | None
    replay_s: float
    dir: Path


class Tool(NamedTuple):
    name: str
    profile: str


def tool_for(run: Run) -> Tool:
    match run:
        case Replay():
            return Tool('trace', 'tools')
        case Fuzz(debug_assertions=debug_assertions):
            return Tool('fuzz', 'dev' if debug_assertions else 'tools')
        case _:
            assert_never(run)


def declared_features(tree_dir, listing):
    listed = next((line.removeprefix('features ') for line in listing.splitlines() if line.startswith('features ')), '')
    names = set(listed.split(',')) - {''}
    declared = tomllib.loads((tree_dir / 'tools' / 'simulator' / 'Cargo.toml').read_text()).get('features', {})
    return sorted(name for name in declared if build_hash.cargo_feature_names([name])[0] in names)


def same_bytes(a, b):
    try:
        return a.read_bytes() == b.read_bytes()
    except OSError:
        return False


def build_tool(root, tree_dir, manifest: Manifest, tool: Tool, target, runner: Runner) -> Path:
    cargo = ['cargo', 'build', '--profile', tool.profile, '-p', 'shell-simulator', '--locked', '--no-default-features']
    cargo += ['--bin', tool.name, '--message-format=json-render-diagnostics']
    if features := declared_features(tree_dir, manifest.listing):
        cargo += ['--features', ','.join(features)]
    here = os.environ.get('IN_NIX_SHELL') and same_bytes(tree_dir / 'flake.lock', root / 'flake.lock')
    argv = cargo if here else ['nix', 'develop', str(tree_dir), '--command', *cargo]
    env = os.environ | {'CARGO_TARGET_DIR': str(target)}
    done = runner(argv, cwd=tree_dir, env=env, stdout=subprocess.PIPE, text=True)
    if done.returncode:
        raise Refused('the recording build did not rebuild; cargo said why on stderr')
    for line in done.stdout.splitlines():
        message = json.loads(line)
        name = message.get('target', {}).get('name')
        if message.get('reason') == 'compiler-artifact' and name == tool.name and message.get('executable'):
            built = Path(message['executable'])
            break
    else:
        raise Refused(f'cargo built no {tool.name}')
    # A replay's build may be the firmware's, which no simulator is; its strict replay relates the two instead.
    if isinstance(manifest.run, Fuzz):
        said = runner([str(built), '--build-hash'], cwd=tree_dir, stdout=subprocess.PIPE, text=True).stdout.strip()
        if said != manifest.build:
            raise Refused(f'the rebuilt {tool.name} is build {said[:12]!r}, not the bundled {manifest.build[:12]}')
    return built


def commit_tree(scratch, manifest: Manifest):
    tree.git('init', '-q', '-b', 'main', cwd=scratch)
    tree.git('add', '-A', '--force', cwd=scratch)
    tree.git(
        *COMMITTER, '-c', 'core.hooksPath=/dev/null', 'commit', '-q', '--no-verify', '-m', manifest.build, cwd=scratch
    )
    listing = scratch / '.local' / 'builds' / manifest.build / 'listing'
    listing.parent.mkdir(parents=True)
    listing.write_text(manifest.listing)


def built_sources(scratch, manifest: Manifest):
    listed = build_hash.read(scratch / '.local' / 'builds', manifest.build)
    if listed is None:
        return False
    found = {}
    for line in tree.git('ls-tree', 'HEAD', '--', *listed.sources, cwd=scratch).splitlines():
        entry, source = line.split('\t')
        found[source] = entry.split()[2]
    return found == listed.sources


def store_pack(bundle, member, packs, expected: PackId) -> Path:
    stored = packs / expected.sha256
    if stored.is_file():
        return stored
    packs.mkdir(parents=True, exist_ok=True)
    staging = staging_file(packs, '.')
    try:
        with bundle.extractfile(member) as source, open(staging, 'wb') as copy:
            shutil.copyfileobj(source, copy)
        if PackId(file_sha256(staging), staging.stat().st_size) != expected:
            raise Refused("the bundle's pack is not the pack its manifest names")
        os.replace(staging, stored)
    finally:
        staging.unlink(missing_ok=True)
    return stored


@contextlib.contextmanager
def held(lock_path):
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with open(lock_path, 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


@dataclass(frozen=True)
class Unpacked:
    manifest: Manifest
    tree_dir: Path
    tool: Path
    run_dir: Path
    rebuilt_s: float | None


def extract_tree_member(bundle, member, scratch, contents):
    """The digest keys the cache, so it covers what lies outside the source roots too, flake.lock among it."""
    safe = tarfile.data_filter(member.replace(name=member.name.removeprefix(TREE)), str(scratch))
    bundle.extract(safe, scratch, filter='fully_trusted')
    data = file_sha256(scratch / safe.name) if safe.isreg() else ''
    fields = [member.name, member.type.decode(), f'{member.mode:o}', member.linkname, data]
    contents.update(b''.join(os.fsencode(field) + b'\0' for field in fields))


def unpack(path, root, runner: Runner) -> Unpacked:
    cache = bundles_dir(root)
    (cache / 'out').mkdir(parents=True, exist_ok=True)
    stem = re.sub(r'[^A-Za-z0-9_-]', '_', path.name.removesuffix('.bundle'))
    run_dir = Path(tempfile.mkdtemp(dir=cache / 'out', prefix=f'{stem}-'))
    (run_dir / 'packs').mkdir()
    scratch = run_dir / 'tree'
    scratch.mkdir()
    try:
        with tarfile.open(path, 'r|zst') as bundle:
            members = iter(bundle)
            first = next(members, None)
            if first is None or first.name != MANIFEST:
                raise Refused(f'{path} does not start with {MANIFEST}; it is not a bundle')
            manifest = parse_manifest(bundle.extractfile(first).read())
            if build_hash.digest(manifest.listing) != manifest.build:
                raise Refused(f"{MANIFEST}'s listing is not build {manifest.build[:12]}'s")
            contents = hashlib.sha256()
            for member in members:
                if member.name.startswith(TREE):
                    extract_tree_member(bundle, member, scratch, contents)
                elif member.name == PACK and manifest.pack is not None:
                    stored = store_pack(bundle, member, cache / 'packs', manifest.pack)
                    (run_dir / 'packs' / manifest.pack.sha256).symlink_to(stored)
                elif member.name in (TRACE, ORIGIN, COURSE):
                    bundle.extract(member, run_dir, filter='data')
                else:
                    raise Refused(f'{path} holds {member.name}, which no bundle holds')
        entry = cache / 'runs' / f'{manifest.build}-{contents.hexdigest()}'
        tree_dir, bin_dir = entry / 'tree', entry / 'bin'
        with held(entry / '.lock'):
            if not tree_dir.is_dir():
                commit_tree(scratch, manifest)
                if not built_sources(scratch, manifest):
                    raise Refused(f"the bundle's tree is not the sources of build {manifest.build[:12]}")
                scratch.rename(tree_dir)
            tool = tool_for(manifest.run)
            installed = bin_dir / tool.profile / tool.name
            rebuilt_s = None
            if not installed.is_file():
                started = time.monotonic()
                installed.parent.mkdir(parents=True, exist_ok=True)
                staging = installed.with_name(f'.{tool.name}')
                built = build_tool(root, tree_dir, manifest, tool, entry / 'target', runner)
                shutil.copy2(built, staging)
                os.replace(staging, installed)
                rebuilt_s = time.monotonic() - started
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    if manifest.pack is not None and not (run_dir / 'packs' / manifest.pack.sha256).exists():
        raise Refused(f'{path} names a pack and holds none')
    return Unpacked(manifest, tree_dir, installed, run_dir, rebuilt_s)


def replay(tool, trace, to_ms, out, cwd, env, runner: Runner):
    argv = [*tool, 'replay', str(trace), '--from', '0', '--to', f'{to_ms}ms', '--every', f'{max(to_ms, 1)}ms']
    argv += ['--out', str(out), '--json', '--no-bundle']
    done = runner(argv, cwd=cwd, env=env, stdout=subprocess.PIPE, text=True)
    try:
        doc = json.loads(done.stdout)
    except ValueError:
        raise Refused(f'the replay printed no JSON (exit {done.returncode})') from None
    if done.returncode or 'error' in doc:
        raise Refused(f'the replay failed: {doc.get("error", f"exit {done.returncode}")}')
    return doc


def parted(doc) -> Faithful | Diverged:
    match doc['parted']:
        case None:
            return Faithful()
        case first:
            return Diverged(first['kind'], first['at_ms'], first['detail'])


def replayed(trace_tool, tree_dir, run_dir, to_ms, env, runner: Runner) -> Outcome:
    doc = replay([str(trace_tool)], run_dir / TRACE, to_ms, run_dir / 'replay', tree_dir, env, runner)
    if doc['mode'] != 'strict':
        raise Refused(f'this rebuild is another build: {doc["why"]}')
    return parted(doc)


def refuzzed(fuzz_tool, tree_dir, run_dir, run: Fuzz, env, runner: Runner) -> Outcome:
    corpus = ['--corpus', str(run_dir / COURSE)] if run.course else ['--no-corpus']
    done = runner(
        [str(fuzz_tool), '--seed', run.seed, *corpus], cwd=tree_dir, env=env, stdout=subprocess.PIPE, text=True
    )
    if done.returncode == 0:
        return NothingFound()
    finding = FINDING.search(done.stdout)
    if done.returncode != 1 or finding is None:
        raise Refused(f'the fuzz session failed (exit {done.returncode}) without a finding')
    return Found(json.loads(finding['check']), finding['at'], finding['detail'])


def run_bundle(path, root, runner: Runner = subprocess.run) -> Ran:
    unpacked = unpack(path, root, runner)
    manifest, run_dir = unpacked.manifest, unpacked.run_dir
    env = os.environ | {'CYCLING_TERRAIN_PACKS': str(run_dir / 'packs')}
    started = time.monotonic()
    match manifest.run:
        case Replay(to_ms):
            got = replayed(unpacked.tool, unpacked.tree_dir, run_dir, to_ms, env, runner)
        case Fuzz() as fuzz:
            got = refuzzed(unpacked.tool, unpacked.tree_dir, run_dir, fuzz, env, runner)
        case _:
            assert_never(manifest.run)
    return Ran(manifest, verdict(manifest.expect, got), unpacked.rebuilt_s, time.monotonic() - started, run_dir)


def ran_json(ran: Ran):
    match ran.verdict:
        case Reproduced():
            name, got = 'reproduced', ran.manifest.expect
        case Differs(_, got):
            name = 'differs'
        case _:
            assert_never(ran.verdict)
    return {
        'verdict': name,
        'expect': outcome_json(ran.manifest.expect),
        'got': outcome_json(got),
        'build': ran.manifest.build,
        'rebuilt_s': ran.rebuilt_s,
        'replay_s': ran.replay_s,
    }


def ran_text(ran: Ran):
    doc = ran_json(ran)
    rebuilt = 'cached' if ran.rebuilt_s is None else f'rebuilt in {ran.rebuilt_s:.1f}s'
    lines = [f'{doc["verdict"]}: build {ran.manifest.build[:12]} ({rebuilt}), ran in {ran.replay_s:.1f}s']
    lines.append(f'  expect {json.dumps(doc["expect"])}')
    if doc['verdict'] == 'differs':
        lines.append(f'  got    {json.dumps(doc["got"])}')
    lines.append(f'  output {ran.dir}')
    return '\n'.join(lines)


def json_arg(parse):
    def parsed(text):
        try:
            return parse(json.loads(text))
        except (ValueError, Refused) as error:
            raise argparse.ArgumentTypeError(str(error)) from error

    return parsed


def arguments(argv):
    rooted = argparse.ArgumentParser(add_help=False, allow_abbrev=False)
    rooted.add_argument('--root', type=Path, default=workspace.ROOT)
    known, argv = rooted.parse_known_args(argv)
    parser = argparse.ArgumentParser(
        prog='cycling bundle', description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument('--json', action='store_true')
    match argv[:1]:
        case ['run']:
            parser.prog += ' run'
            parser.add_argument('file', type=Path)
        case ['write']:
            parser.prog += ' write'
            parser.add_argument('--out', type=Path, required=True)
            parser.add_argument('--trace', type=Path, required=True)
            parser.add_argument('--build', required=True)
            parser.add_argument('--run', type=json_arg(parse_run), required=True)
            parser.add_argument('--expect', type=json_arg(parse_expect), required=True)
            parser.add_argument('--course', type=Path)
            parser.add_argument('--pack', type=Path)
        case _:
            parser.add_argument('trace', type=Path)
            parser.add_argument('--out', type=Path)
    command = argv[0] if argv[:1] in (['run'], ['write']) else 'bundle'
    args = parser.parse_args(argv[1:] if command != 'bundle' else argv)
    args.root = known.root
    return command, args


def main(argv=None, runner: Runner = subprocess.run):
    command, args = arguments(sys.argv[1:] if argv is None else argv)
    try:
        match command:
            case 'run':
                ran = run_bundle(args.file.resolve(), args.root, runner)
                print(json.dumps(ran_json(ran)) if args.json else ran_text(ran))
                return 0 if isinstance(ran.verdict, Reproduced) else 1
            case 'write':
                course, pack = (path and path.resolve() for path in (args.course, args.pack))
                out = write(
                    args.root, args.out.resolve(), args.trace.resolve(), args.build, args.run, args.expect, course, pack
                )
            case _:
                out = bundle_trace(args.root, args.trace.resolve(), args.out and args.out.resolve(), runner)
    except Refused as error:
        if args.json:
            print(json.dumps({'error': str(error)}))
        print(f'cycling bundle: {error}', file=sys.stderr)
        return 2
    print(json.dumps({'bundle': str(out)}) if args.json else out)
    return 0


if __name__ == '__main__':
    sys.exit(main())
