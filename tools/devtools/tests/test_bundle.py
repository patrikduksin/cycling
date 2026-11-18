import hashlib
import io
import json
import shutil
import subprocess
import tarfile
from pathlib import Path

import pytest

from cycling_devtools import build_hash, bundle
from cycling_devtools.bundle import Differs, Diverged, Faithful, Found, Fuzz, NothingFound, Replay, Reproduced

CARGO_TOML = '[package]\nname = "shell-simulator"\n\n[features]\ndefault = []\nlive = []\nplanted-bug = []\n'


def git(cwd, *args):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def repo(tmp_path):
    root = tmp_path / 'repo'
    (root / 'apps').mkdir(parents=True)
    (root / 'tools' / 'simulator').mkdir(parents=True)
    (root / 'tools' / 'simulator' / 'build-sources').write_text('apps\nCargo.lock\n')
    (root / 'tools' / 'simulator' / 'Cargo.toml').write_text(CARGO_TOML)
    (root / 'apps' / 'app.rs').write_text('fn main() {}\n')
    (root / 'apps' / 'run.sh').write_text('#!/bin/sh\n')
    (root / 'apps' / 'run.sh').chmod(0o755)
    (root / 'apps' / 'link').symlink_to('app.rs')
    (root / 'Cargo.lock').write_text('version = 4\n')
    (root / 'flake.lock').write_text('{"nodes": {}}\n')
    (root / '.gitignore').write_text('/.local/\n*.tmp\n')
    (root / 'kept.tmp').write_text('tracked though ignored\n')
    git(root, 'init', '-q', '-b', 'main')
    git(root, 'add', '-A')
    git(root, 'add', '-f', 'kept.tmp')
    git(root, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '-m', 'first')
    return root


@pytest.fixture
def build(repo, tmp_path, capsys):
    rustc = tmp_path / 'rustc'
    rustc.write_text('#!/bin/sh\necho "rustc 1.100.0"\n')
    rustc.chmod(0o755)

    def built(features=''):
        argv = ['--root', str(repo), 'hash', '--scope', 'tree', '--rustc', str(rustc), '--features', features]
        assert build_hash.main(argv) == 0
        return capsys.readouterr().out.split()[0]

    return built


@pytest.fixture
def trace(tmp_path):
    path = tmp_path / 'ride' / 'morning.trace'
    path.parent.mkdir()
    path.write_bytes(b'VANA-TRACE world log')
    return path


def members(path):
    with tarfile.open(path, 'r:zst') as opened:
        return {m.name: (opened.extractfile(m).read() if m.isfile() else m) for m in opened.getmembers()}


def manifest_of(path):
    with tarfile.open(path, 'r:zst') as opened:
        assert opened.getnames()[0] == 'bundle.json', 'the manifest comes first, so a run reads it without the tree'
        return bundle.parse_manifest(opened.extractfile('bundle.json').read())


def extracted_tree(path, into):
    with tarfile.open(path, 'r:zst') as opened:
        inside = [
            m.replace(name=m.name.removeprefix('tree/')) for m in opened.getmembers() if m.name.startswith('tree/')
        ]
        for member in inside:
            opened.extract(member, into, filter='data')
    git(into, 'init', '-q')
    git(into, 'add', '-A', '--force')
    return git(into, 'write-tree')


def entry(repo, build):
    (tree_dir,) = (repo / '.local' / 'bundles' / 'runs').glob(f'{build}-*/tree')
    return tree_dir.parent


def listed_tree(repo, build):
    return (repo / '.local' / 'builds' / build / 'listing').read_text().splitlines()[0].removeprefix('tree ')


@pytest.mark.parametrize(
    ('expect', 'got', 'reproduced'),
    [
        (Faithful(), Faithful(), True),
        (Faithful(), Diverged('Refused', 1200, 'read 4'), False),
        (Diverged('Refused', 1200, 'read 4'), Diverged('Refused', 1200, 'read 4'), True),
        (Diverged('Refused', 1200, 'read 4'), Diverged('Refused', 1300, 'read 4'), False),
        (Diverged('Refused', 1200, 'read 4'), Diverged('Unrecorded', 1200, 'read 4'), False),
        (Diverged('Refused', 1200, 'read 4'), Diverged('Refused', 1200, 'read 5'), False),
        (Diverged('Refused', 1200, 'read 4'), Faithful(), False),
        (Found('PowerShown', 'step 12', 'shown 0 W'), Found('PowerShown', 'step 12', 'shown 0 W'), True),
        (Found('PowerShown', 'step 12', 'shown 0 W'), Found('PowerShown', 'end', 'shown 0 W'), False),
        (Found('PowerShown', 'step 12', 'shown 0 W'), Found('ToastStuck', 'step 12', 'shown 0 W'), False),
        (Found('PowerShown', 'step 12', 'shown 0 W'), Found('PowerShown', 'step 12', 'shown 9 W'), False),
        (Found('PowerShown', 'step 12', 'shown 0 W'), NothingFound(), False),
    ],
)
def test_the_verdict_is_reproduced_exactly_when_the_outcome_is_the_expectation(expect, got, reproduced):
    assert bundle.verdict(expect, got) == (Reproduced() if reproduced else Differs(expect, got))


def test_a_manifest_parses_back_to_itself():
    manifest = bundle.Manifest(
        1, 'a' * 40, 'source apps 1\n', bundle.PackId('b' * 64, 9), Fuzz('0x1f', True, True), Found('X', 'setup', 'why')
    )
    assert bundle.parse_manifest(json.dumps(bundle.manifest_json(manifest))) == manifest


@pytest.mark.parametrize(
    ('change', 'refusal'),
    [
        ({'format': 2}, 'this bundle is format 2; this checkout runs format 1'),
        ({'build': 'abc'}, "names build 'abc', not 40 hex digits"),
        ({'run': {'kind': 'replay', 'to_ms': -1}}, 'run is'),
        ({'run': {'kind': 'bisect'}}, 'run is'),
        ({'expect': {'faithful': False}}, 'expect is'),
        ({'pack': {'sha256': 'b' * 64}}, 'pack is null or'),
    ],
)
def test_a_manifest_outside_format_1_is_refused(change, refusal):
    raw = {'format': 1, 'build': 'a' * 40, 'listing': '', 'pack': None, 'run': {'kind': 'replay', 'to_ms': 5}}
    raw |= {'expect': {'faithful': True}} | change
    with pytest.raises(bundle.Refused, match=refusal):
        bundle.parse_manifest(json.dumps(raw))


def test_a_clean_build_bundles_its_commit_tree(repo, build, trace, tmp_path):
    name = build()
    out = bundle.write(repo, tmp_path / 'out.bundle', trace, name, Replay(5000), Faithful())
    manifest = manifest_of(out)
    assert manifest == bundle.Manifest(
        1, name, (repo / '.local' / 'builds' / name / 'listing').read_text(), None, Replay(5000), Faithful()
    )
    held = members(out)
    assert held['trace'] == b'VANA-TRACE world log'
    assert {'trace.origin', 'course.trace', 'pack'}.isdisjoint(held)
    assert extracted_tree(out, tmp_path / 'extracted') == git(repo, 'rev-parse', 'HEAD^{tree}')


def test_a_dirty_build_bundles_the_tree_it_built_not_the_checkout_now(repo, build, trace, tmp_path):
    (repo / 'apps' / 'app.rs').write_text('fn main() { edited }\n')
    (repo / 'apps' / 'new.rs').write_text('untracked\n')
    (repo / 'apps' / 'run.sh').chmod(0o644)
    name = build()
    git(repo, 'checkout', '-q', '--', '.')
    (repo / 'apps' / 'new.rs').unlink()
    out = bundle.write(repo, tmp_path / 'out.bundle', trace, name, Replay(5000), Faithful())
    extracted = tmp_path / 'extracted'
    assert extracted_tree(out, extracted) == listed_tree(repo, name)
    assert (extracted / 'apps' / 'new.rs').read_text() == 'untracked\n'
    assert (extracted / 'kept.tmp').exists(), 'a tracked file the tree ignores'
    assert (extracted / 'apps' / 'link').readlink() == Path('app.rs')


def test_a_bundled_tree_ignores_export_attributes(repo, build, trace, tmp_path):
    (repo / 'apps' / 'stamp.rs').write_text('// $Format:%H$\n')
    git(repo, 'add', 'apps/stamp.rs')
    git(repo, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '-m', 'stamp')
    (repo / '.git' / 'info' / 'attributes').write_text('apps/app.rs export-ignore\napps/stamp.rs export-subst\n')
    out = bundle.write(repo, tmp_path / 'out.bundle', trace, build(), Replay(1), Faithful())
    extracted = tmp_path / 'extracted'
    assert extracted_tree(out, extracted) == git(repo, 'rev-parse', 'HEAD^{tree}')
    assert (extracted / 'apps' / 'app.rs').read_text() == 'fn main() {}\n'
    assert (extracted / 'apps' / 'stamp.rs').read_text() == '// $Format:%H$\n'
    assert (extracted / 'apps' / 'run.sh').stat().st_mode & 0o111, 'an executable stays executable'


def test_a_build_not_archived_here_is_refused(repo, trace, tmp_path):
    with pytest.raises(bundle.Refused, match=f'build {"f" * 40} is not archived here; bundle it on the machine'):
        bundle.write(repo, tmp_path / 'out.bundle', trace, 'f' * 40, Replay(1), Faithful())


def test_the_trace_origin_and_course_travel_with_a_fuzz_finding(repo, build, trace, tmp_path):
    trace.with_suffix('.origin').write_bytes(b'VANA-ORIGIN2')
    course = tmp_path / 'course.trace'
    course.write_bytes(b'course')
    run = Fuzz('0x00000000000000ab', True, False)
    out = bundle.write(repo, tmp_path / 'f.bundle', trace, build(), run, Found('X', 'step 3', 'why'), course=course)
    held = members(out)
    assert (held['trace.origin'], held['course.trace']) == (b'VANA-ORIGIN2', b'course')
    with pytest.raises(bundle.Refused, match='exactly when --course'):
        bundle.write(repo, tmp_path / 'g.bundle', trace, build(), run, Found('X', 'step 3', 'why'))


class Summary:
    def __init__(self, header, replay=None):
        self.header = header
        self.replay = replay or {'mode': 'strict', 'why': 'same build', 'parted': None}
        self.replays = []

    def __call__(self, argv, **kwargs):
        if argv[2] == 'replay':
            self.replays.append(argv)
            return subprocess.CompletedProcess(argv, 0, json.dumps(self.replay))
        assert argv[2:4] == ['trace', 'summary'] and argv[-1] == '--json'
        return subprocess.CompletedProcess(argv, 0, json.dumps({'header': self.header}))


def test_a_trace_bundles_the_pack_its_header_names_by_length_and_digest(repo, build, trace, tmp_path, monkeypatch):
    store = tmp_path / 'packs'
    store.mkdir()
    (store / 'a-decoy.pack').write_bytes(b'TERRAIN-other')
    (store / 'b-real.pack').write_bytes(b'TERRAIN-alpes')
    (store / 'c-short.pack').write_bytes(b'TERRAIN')
    monkeypatch.setenv('CYCLING_TERRAIN_PACKS', str(store))
    sha256 = hashlib.sha256(b'TERRAIN-alpes').hexdigest()
    name = build()
    header = {'build': name, 'end_ms': 61000, 'terrain': {'sha256': sha256, 'length': 13, 'ready': True}}
    out = bundle.bundle_trace(repo, trace, None, Summary(header))
    assert out == repo / '.local' / 'bundles' / f'morning-{name[:12]}.bundle'
    manifest = manifest_of(out)
    assert (manifest.pack, manifest.run, manifest.expect) == (bundle.PackId(sha256, 13), Replay(61000), Faithful())
    assert members(out)['pack'] == b'TERRAIN-alpes'

    not_ready = header | {'terrain': header['terrain'] | {'ready': False}}
    assert manifest_of(bundle.bundle_trace(repo, trace, tmp_path / 'n.bundle', Summary(not_ready))).pack is None
    missing = header | {'terrain': header['terrain'] | {'sha256': 'c' * 64}}
    with pytest.raises(bundle.Refused, match=f'no file in {store} is that pack'):
        bundle.bundle_trace(repo, trace, tmp_path / 'm.bundle', Summary(missing))


def test_a_trace_bundles_what_its_strict_replay_shows_here(repo, build, trace, tmp_path):
    name = build()
    header = {'build': name, 'end_ms': 61000, 'terrain': None}
    faithful = Summary(header)
    assert manifest_of(bundle.bundle_trace(repo, trace, tmp_path / 'f.bundle', faithful)).expect == Faithful()
    ((_, script, *argv),) = faithful.replays
    assert script == str(repo / 'tools' / 'simulator' / 'trace.sh')
    assert argv[:7] == ['replay', str(trace), '--from', '0', '--to', '61000ms', '--every']
    assert argv[7:8] == ['61000ms'] and argv[-2:] == ['--json', '--no-bundle']

    first = {'kind': 'Unrecorded', 'at_ms': 3300, 'detail': 'a host command the trace never saw'}
    diverged = Summary(header, {'mode': 'strict', 'why': 'same build', 'parted': first})
    expect = manifest_of(bundle.bundle_trace(repo, trace, tmp_path / 'd.bundle', diverged)).expect
    assert expect == Diverged('Unrecorded', 3300, 'a host command the trace never saw')

    other = Summary(header, {'mode': 'stimulus', 'why': 'recorded by another build', 'parted': None})
    with pytest.raises(bundle.Refused, match=r'in stimulus mode \(recorded by another build\).*whose build matches'):
        bundle.bundle_trace(repo, trace, tmp_path / 'o.bundle', other)
    assert not (tmp_path / 'o.bundle').exists()


class Twin:
    def __init__(self, tmp_path, replay=None, fuzz=(0, 'pass\n'), build_hash=None):
        self.bin = tmp_path / 'cargo-out'
        self.calls = []
        self.replay = replay or {'mode': 'strict', 'why': 'same build', 'parted': None}
        self.fuzz = fuzz
        self.build_hash = build_hash

    def __call__(self, argv, cwd, env=None, **kwargs):
        self.calls.append((argv, Path(cwd), env))
        program = Path(argv[0]).name
        if program in ('cargo', 'nix'):
            self.bin.mkdir(exist_ok=True)
            name = argv[argv.index('--bin') + 1]
            (self.bin / name).write_text(f'#!/bin/sh\necho {name}\n')
            artifact = {'name': name, 'kind': ['bin']}
            lines = [{'reason': 'compiler-artifact', 'target': artifact, 'executable': str(self.bin / name)}]
            lines.append({'reason': 'build-finished', 'success': True})
            return subprocess.CompletedProcess(argv, 0, ''.join(json.dumps(line) + '\n' for line in lines))
        if argv[1:] == ['--build-hash']:
            (listing,) = (Path(cwd) / '.local' / 'builds').glob('*/listing')
            return subprocess.CompletedProcess(
                argv, 0, f'{self.build_hash or build_hash.digest(listing.read_text())}\n'
            )
        if program == 'trace':
            return subprocess.CompletedProcess(argv, 0, json.dumps(self.replay))
        if program == 'fuzz':
            return subprocess.CompletedProcess(argv, *self.fuzz)
        raise AssertionError(argv)

    def builds(self):
        return [call for call in self.calls if Path(call[0][0]).name in ('cargo', 'nix')]

    def ran(self, program):
        return [call for call in self.calls if Path(call[0][0]).name == program and call[0][1:] != ['--build-hash']]


@pytest.fixture
def in_shell(monkeypatch):
    monkeypatch.setenv('IN_NIX_SHELL', 'impure')


def test_a_run_rebuilds_the_build_once_and_replays_from_the_bundle_alone(repo, build, trace, tmp_path, in_shell):
    pack = tmp_path / 'alpes.pack'
    pack.write_bytes(b'TERRAIN-alpes')
    name = build(features='default,planted-bug')
    out = bundle.write(repo, tmp_path / 'x.bundle', trace, name, Replay(61000), Faithful(), pack=pack)
    trace.unlink()
    pack.unlink()
    twin = Twin(tmp_path)

    first = bundle.run_bundle(out, repo, twin)
    cache = repo / '.local' / 'bundles'
    tree_dir = entry(repo, name) / 'tree'
    ((cargo, cwd, env),), (replay,) = twin.builds(), twin.ran('trace')
    assert cargo == [
        'cargo', 'build', '--profile', 'tools', '-p', 'shell-simulator', '--locked', '--no-default-features',
        '--bin', 'trace', '--message-format=json-render-diagnostics', '--features', 'default,planted-bug',
    ]  # fmt: skip
    assert (cwd, env['CARGO_TARGET_DIR']) == (tree_dir, str(entry(repo, name) / 'target'))
    assert git(tree_dir, 'rev-parse', 'HEAD^{tree}') == listed_tree(repo, name), 'the commit is the built tree'
    assert (tree_dir / '.local' / 'builds' / name / 'listing').read_text() == manifest_of(out).listing
    assert git(tree_dir, 'status', '--porcelain') == ''

    run_dir = first.dir
    assert (run_dir.parent, run_dir.name[:2]) == (cache / 'out', 'x-')
    argv, cwd, env = replay
    assert argv == [
        str(entry(repo, name) / 'bin' / 'tools' / 'trace'), 'replay', str(run_dir / 'trace'), '--from', '0',
        '--to', '61000ms', '--every', '61000ms', '--out', str(run_dir / 'replay'), '--json', '--no-bundle',
    ]  # fmt: skip
    assert (run_dir / 'trace').read_bytes() == b'VANA-TRACE world log'
    packs = Path(env['CYCLING_TERRAIN_PACKS'])
    sha256 = hashlib.sha256(b'TERRAIN-alpes').hexdigest()
    assert [p.name for p in packs.iterdir()] == [sha256]
    assert (packs / sha256).read_bytes() == b'TERRAIN-alpes'
    assert (packs / sha256).resolve() == cache / 'packs' / sha256
    assert first.verdict == Reproduced()
    assert first.rebuilt_s is not None

    again = bundle.run_bundle(out, repo, twin)
    assert len(twin.builds()) == 1, 'the second run reuses the build'
    assert again.rebuilt_s is None
    assert again.verdict == Reproduced()


@pytest.mark.parametrize('file_name', ['...bundle', 'tree.bundle', 'bin.bundle', '..bundle'])
def test_a_bundle_file_name_never_reaches_outside_its_own_run(repo, build, trace, tmp_path, in_shell, file_name):
    name = build()
    out = bundle.write(repo, tmp_path / file_name, trace, name, Replay(1), Faithful())
    cache = repo / '.local' / 'bundles'
    keep = [cache / 'runs' / 'kept-by-another-build']
    for path in keep:
        path.mkdir(parents=True)
    twin = Twin(tmp_path)
    ran = [bundle.run_bundle(out, repo, twin) for _ in range(2)]
    assert all(path.is_dir() for path in keep)
    assert all(r.dir.parent == cache / 'out' for r in ran) and ran[0].dir != ran[1].dir
    assert len(twin.builds()) == 1, 'the cached tree and binary survive the second run'
    assert git(entry(repo, name) / 'tree', 'status', '--porcelain') == ''


def test_two_runs_of_one_build_from_same_named_files_each_replay_their_own_trace(
    repo, build, trace, tmp_path, in_shell
):
    name = build()
    bundles = []
    for side in ('a', 'b'):
        trace.write_bytes(f'VANA-TRACE {side}'.encode())
        (tmp_path / side).mkdir()
        bundles.append(bundle.write(repo, tmp_path / side / 'failure.bundle', trace, name, Replay(1), Faithful()))
    twin = Twin(tmp_path)
    seen = []

    def runner(argv, cwd, env=None, **kwargs):
        if Path(argv[0]).name == 'trace' and argv[1] == 'replay':
            replayed = Path(argv[2])
            if not seen:
                seen.append(None)
                bundle.run_bundle(bundles[1], repo, runner)
            seen.append(replayed.read_bytes())
        return twin(argv, cwd, env, **kwargs)

    bundle.run_bundle(bundles[0], repo, runner)
    assert seen[1:] == [b'VANA-TRACE b', b'VANA-TRACE a'], 'the second run left the first one its trace'


def tampered(path, out, change):
    with tarfile.open(path, 'r:zst') as source, tarfile.open(out, 'w:zst') as copy:
        for member in source.getmembers():
            data = source.extractfile(member).read() if member.isfile() else None
            if data is not None:
                data = change(member.name, data)
                member.size = len(data)
            copy.addfile(member, None if data is None else io.BytesIO(data))
    return out


def test_a_tree_that_is_not_the_build_is_refused_and_never_cached(repo, build, trace, tmp_path, in_shell):
    name = build()
    real = bundle.write(repo, tmp_path / 'real.bundle', trace, name, Replay(1), Faithful())
    edited = tampered(
        real, tmp_path / 'edited.bundle', lambda n, d: b'fn main() { evil }\n' if n == 'tree/apps/app.rs' else d
    )
    with pytest.raises(bundle.Refused, match=f"the bundle's tree is not the sources of build {name[:12]}"):
        bundle.run_bundle(edited, repo, Twin(tmp_path))
    assert bundle.run_bundle(real, repo, Twin(tmp_path)).verdict == Reproduced()
    tree_dir = entry(repo, name) / 'tree'
    assert (tree_dir / 'apps' / 'app.rs').read_text() == 'fn main() {}\n', 'the refused tree poisoned no cache'
    with pytest.raises(bundle.Refused, match="the bundle's tree is not the sources"):
        bundle.run_bundle(edited, repo, Twin(tmp_path))

    def relisted(member, data):
        if member != 'bundle.json':
            return data
        doc = json.loads(data)
        doc['listing'] = doc['listing'].replace('features ', 'features planted_bug', 1)
        return json.dumps(doc).encode()

    with pytest.raises(bundle.Refused, match=f"bundle.json's listing is not build {name[:12]}'s"):
        bundle.run_bundle(tampered(real, tmp_path / 'relisted.bundle', relisted), repo, Twin(tmp_path))


def test_another_dev_shell_builds_through_nix_develop(repo, build, trace, tmp_path, in_shell):
    out = bundle.write(repo, tmp_path / 'x.bundle', trace, build(), Replay(1000), Faithful())
    (repo / 'flake.lock').write_text('{"nodes": {"newer": {}}}\n')
    twin = Twin(tmp_path)
    bundle.run_bundle(out, repo, twin)
    ((argv, cwd, _),) = twin.builds()
    assert argv[:4] == ['nix', 'develop', str(cwd), '--command']
    assert argv[4:6] == ['cargo', 'build']


def test_a_divergence_other_than_the_expected_one_differs(repo, build, trace, tmp_path, in_shell, capsys):
    expect = Diverged('Refused', 1200, 'read 4')
    out = bundle.write(repo, tmp_path / 'x.bundle', trace, build(), Replay(2000), expect)
    parted = {'kind': 'Refused', 'at_ms': 1300, 'detail': 'read 4'}
    twin = Twin(tmp_path, replay={'mode': 'strict', 'why': '', 'parted': parted})
    assert bundle.main(['--root', str(repo), 'run', str(out), '--json'], runner=twin) == 1
    doc = json.loads(capsys.readouterr().out)
    assert (doc['verdict'], doc['expect'], doc['got']) == (
        'differs',
        {'diverged': {'kind': 'Refused', 'at_ms': 1200, 'detail': 'read 4'}},
        {'diverged': {'kind': 'Refused', 'at_ms': 1300, 'detail': 'read 4'}},
    )


def test_a_rebuild_the_twin_does_not_call_the_recording_build_is_an_error(
    repo, build, trace, tmp_path, in_shell, capsys
):
    out = bundle.write(repo, tmp_path / 'x.bundle', trace, build(), Replay(2000), Faithful())
    twin = Twin(tmp_path, replay={'mode': 'stimulus', 'why': 'builds differ', 'parted': None})
    assert bundle.main(['--root', str(repo), 'run', str(out), '--json'], runner=twin) == 2
    assert json.loads(capsys.readouterr().out) == {'error': 'this rebuild is another build: builds differ'}


def test_a_fuzz_finding_reruns_its_seed_on_its_course(repo, build, trace, tmp_path, in_shell):
    course = tmp_path / 'course.trace'
    course.write_bytes(b'course')
    found = Found('no panic', 'step 12', 'index out of bounds at apps/vana/src/ride/log.rs:88')
    run = Fuzz('0x00000000000000ab', True, False)
    out = bundle.write(repo, tmp_path / 'finding.bundle', trace, build(), run, found, course=course)
    line = 'FAIL seed=0x00000000000000ab check="no panic" at=step 12 t=0:01:02\n  {}\n  world log 0123\n'
    twin = Twin(tmp_path, fuzz=(1, line.format(found.detail)))
    ran = bundle.run_bundle(out, repo, twin)
    (cargo, _, _), ((argv, _, _),) = twin.builds()[0], twin.ran('fuzz')
    assert cargo[2:4] == ['--profile', 'tools'] and cargo[cargo.index('--bin') + 1] == 'fuzz'
    assert argv[1:] == ['--seed', '0x00000000000000ab', '--corpus', str(ran.dir / 'course.trace')]
    assert (ran.dir / 'course.trace').read_bytes() == b'course'
    assert ran.verdict == Reproduced()
    assert bundle.run_bundle(out, repo, Twin(tmp_path, fuzz=(0, 'pass\n'))).verdict == Differs(found, NothingFound())
    other = 'attempt to subtract with overflow at apps/vana/src/ride/log.rs:91'
    assert bundle.run_bundle(out, repo, Twin(tmp_path, fuzz=(1, line.format(other)))).verdict == Differs(
        found, Found('no panic', 'step 12', other)
    ), 'another panic at the same step is another finding'


def test_a_bundle_names_its_file_on_the_command_line(repo, build, trace, tmp_path, capsys):
    name = build()
    argv = ['--root', str(repo), 'write', '--out', str(tmp_path / 'w.bundle'), '--trace', str(trace)]
    argv += ['--build', name, '--run', '{"kind": "replay", "to_ms": 3}', '--expect', '{"faithful": true}']
    assert bundle.main(argv) == 0
    assert capsys.readouterr().out.strip() == str(tmp_path / 'w.bundle')
    assert manifest_of(tmp_path / 'w.bundle').run == Replay(3)


def test_a_finding_made_with_debug_assertions_reruns_on_a_build_that_checks_them(
    repo, build, trace, tmp_path, in_shell
):
    found = Found('no panic', 'step 3', 'a debug_assert! failed at apps/vana/src/ride/log.rs:40')
    out = bundle.write(repo, tmp_path / 'finding.bundle', trace, build(), Fuzz('0x5', False, True), found)
    line = f'FAIL seed=0x0000000000000005 check="no panic" at=step 3 t=0:00:09\n  {found.detail}\n'
    twin = Twin(tmp_path, fuzz=(1, line))
    assert bundle.run_bundle(out, repo, twin).verdict == Reproduced()
    ((cargo, _, _),) = twin.builds()
    assert cargo[2:4] == ['--profile', 'dev'], 'the dev profile keeps debug_assertions on'
    assert twin.ran('fuzz')[0][0][0].endswith('/bin/dev/fuzz')


def test_a_rebuild_that_is_another_build_is_refused_and_not_kept(repo, build, trace, tmp_path, in_shell, capsys):
    name = build()
    out = bundle.write(repo, tmp_path / 'x.bundle', trace, name, Fuzz('0x5', False, False), Found('X', 'end', ''))
    assert (
        bundle.main(['--root', str(repo), 'run', str(out), '--json'], runner=Twin(tmp_path, build_hash='e' * 40)) == 2
    )
    refusal = f"the rebuilt fuzz is build 'eeeeeeeeeeee', not the bundled {name[:12]}"
    assert json.loads(capsys.readouterr().out) == {'error': refusal}
    twin = Twin(tmp_path, fuzz=(1, 'FAIL seed=0x5 check="X" at=end t=0:00:01\n  \n'))
    assert bundle.run_bundle(out, repo, twin).verdict == Reproduced()
    assert len(twin.builds()) == 1, 'the refused binary was never cached'


def test_a_replay_rebuild_is_not_held_to_the_recording_build_hash(repo, build, trace, tmp_path, in_shell):
    out = bundle.write(repo, tmp_path / 'device.bundle', trace, build(), Replay(1000), Faithful())
    twin = Twin(tmp_path, build_hash='e' * 40)
    assert bundle.run_bundle(out, repo, twin).verdict == Reproduced(), 'a device build is never a simulator build'


def test_bundles_of_one_build_with_other_trees_never_share_a_rebuild(repo, build, trace, tmp_path, in_shell):
    real = bundle.write(repo, tmp_path / 'real.bundle', trace, build(), Replay(1), Faithful())
    locked = tampered(
        real, tmp_path / 'locked.bundle', lambda n, d: b'{"nodes": {"newer": {}}}\n' if n == 'tree/flake.lock' else d
    )
    twin = Twin(tmp_path)
    for path in (real, locked, real, locked):
        assert bundle.run_bundle(path, repo, twin).verdict == Reproduced()
    assert [Path(argv[0]).name for argv, _, _ in twin.builds()] == ['cargo', 'nix'], (
        'each tree rebuilds once, its own way'
    )


WORKSPACE = {
    'Cargo.toml': '[workspace]\nmembers = ["apps/vana", "tools/simulator"]\nresolver = "2"\n\n'
    '[profile.tools]\ninherits = "dev"\n',
    'apps/vana/Cargo.toml': '[package]\nname = "vana"\nversion = "0.0.0"\nedition = "2021"\n',
    'tools/simulator/Cargo.toml': '[package]\nname = "shell-simulator"\nversion = "0.0.0"\nedition = "2021"\n\n'
    '[dependencies]\nvana = { path = "../../apps/vana" }\n\n[features]\ndefault = []\n\n'
    '[[bin]]\nname = "fuzz"\npath = "fuzz.rs"\n',
    'tools/simulator/fuzz.rs': 'fn main() {\n    println!("FAIL seed=0x1 check=\\"X\\" at=end t=0:00:01\\n  {}", vana::FOUND);\n'
    '    std::process::exit(1);\n}\n',
    'tools/simulator/build-sources': 'apps\ntools/simulator\nCargo.toml\nCargo.lock\n',
    'flake.lock': '{"nodes": {}}\n',
    '.gitignore': '/.local/\n/target/\n',
}


@pytest.mark.skipif(shutil.which('cargo') is None, reason='cargo is not installed')
def test_two_builds_rebuilt_back_to_back_each_run_their_own_code(tmp_path, in_shell, capsys):
    root = tmp_path / 'repo'
    for name, text in WORKSPACE.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)
    trace = tmp_path / 'morning.trace'
    trace.write_bytes(b'VANA-TRACE')
    rustc = tmp_path / 'rustc'
    rustc.write_text('#!/bin/sh\necho "rustc 1.100.0"\n')
    rustc.chmod(0o755)
    git(root, 'init', '-q', '-b', 'main')
    bundles = []
    for found in ('one', 'two'):
        (root / 'apps' / 'vana' / 'src').mkdir(parents=True, exist_ok=True)
        (root / 'apps' / 'vana' / 'src' / 'lib.rs').write_text(f'pub const FOUND: &str = "{found}";\n')
        subprocess.run(['cargo', 'generate-lockfile', '--offline'], cwd=root, check=True, capture_output=True)
        git(root, 'add', '-A')
        git(root, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '-m', found)
        assert build_hash.main(['--root', str(root), 'hash', '--scope', 'tree', '--rustc', str(rustc)]) == 0
        name = capsys.readouterr().out.split()[0]
        run = Fuzz('0x1', False, False)
        bundles.append(bundle.write(root, tmp_path / f'{found}.bundle', trace, name, run, Found('X', 'end', found)))

    def runner(argv, cwd, **kwargs):
        if argv[1:] == ['--build-hash']:
            (listing,) = (Path(cwd) / '.local' / 'builds').glob('*/listing')
            return subprocess.CompletedProcess(argv, 0, f'{build_hash.digest(listing.read_text())}\n')
        return subprocess.run(argv, cwd=cwd, **kwargs)

    for path in bundles:
        assert bundle.run_bundle(path, root, runner).verdict == Reproduced(), f'{path.name} ran its own vana'
