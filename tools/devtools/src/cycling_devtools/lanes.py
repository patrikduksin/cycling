import os
import tomllib
from dataclasses import dataclass


@dataclass(frozen=True)
class Gate:
    name: str
    command: str
    env: tuple[tuple[str, str], ...] = ()
    test: bool = False
    reports: bool = False
    # A command that prints the files outside the tree the gate judges, such as the private rides, one per line.
    # A cached pass holds only for the same files with the same contents.
    reads: str | None = None


BUILD = Gate('build', 'cycling build', (('CYCLING_IMAGE_DIR', '.local/verify/images/build'),))

# Lanes that share a cargo target dir queue on its lock, so clippy gets its own.
LANES = (
    (BUILD,),
    (
        Gate(
            'test-rust',
            'cycling test-rust',
            (('NEXTEST_PROFILE', 'without-ride-scenarios'),),
            test=True,
            reads='cycling corpus --list-fixtures',
        ),
        Gate('simulate', 'cycling simulate'),
    ),
    (Gate('test-trouble-host', 'cycling test-trouble-host', test=True),),
    (Gate('test-python', 'cycling test-python', test=True),),
    # Both start with bun install, which two at once in a fresh worktree leave half done.
    (Gate('test-web', 'cycling test-web', test=True), Gate('check-web', 'cycling check-web')),
    (Gate('check-rust', 'cycling check-rust', (('CARGO_TARGET_DIR', 'target/clippy'),)),),
    (
        Gate('check-boundaries', 'cycling check-boundaries'),
        Gate('check-devices', 'cycling check-devices'),
        Gate('check-generated-web', 'cycling check-generated-web'),
        Gate('check-python', 'cycling check-python'),
    ),
    (Gate('screens', 'cycling sim-compare', reports=True),),
    (Gate('corpus', 'cycling corpus', reads='cycling corpus --list'),),
)
ALONE = (Gate('harness-virtual', 'cycling harness-virtual'),)
DEEP = (
    Gate('corpus-all', 'nice -n 19 cycling corpus --all', reads='cycling corpus --all --list'),
    Gate('fuzz', 'nice -n 19 cycling fuzz --fixed'),
    Gate('planted', 'nice -n 19 cycling fuzz-planted'),
    Gate('fixtures-planted', 'nice -n 19 cycling fixtures-planted'),
    Gate(
        'explore',
        'nice -n 19 cargo nextest run --locked --no-default-features -p vana --lib --run-ignored only'
        " -E 'test(/^ride::course::follow::tests::explore::deep::/)'",
    ),
)

EVERY = frozenset(gate.name for lane in (*LANES, ALONE) for gate in lane)
NONE = frozenset()
RUST_CODE = frozenset(
    {'test-rust', 'simulate', 'check-rust', 'check-boundaries', 'screens', 'harness-virtual', 'test-python', 'corpus'}
)
RUST_BUILD = RUST_CODE | {'build', 'check-devices'}
WEB = frozenset({'test-web', 'check-web'})
# check-boundaries fails a document that names a missing file, and a document can name any path.
ALWAYS = frozenset({'check-boundaries'})

VERIFY_MODULES = ('admission', 'clean', 'cli', 'lanes', 'runner', 'tree', 'verify', 'workspace')

# A path no rule matches selects every gate, so a new directory is verified in full until it gets a rule.
RULES = (
    ('apps/', RUST_CODE | {'check-generated-web'}),
    ('packages/', RUST_CODE),
    ('devices/', RUST_BUILD),
    ('vendor/', RUST_BUILD | {'test-trouble-host'}),
    ('Cargo.toml', RUST_BUILD),
    ('Cargo.lock', RUST_BUILD),
    ('.cargo/', RUST_BUILD | {'test-trouble-host'}),
    ('.config/nextest.toml', RUST_BUILD),
    ('tools/simulator/', RUST_CODE),
    *((f'tools/devtools/src/cycling_devtools/{module}.py', EVERY) for module in VERIFY_MODULES),
    ('tools/devtools/src/cycling_devtools/build_hash.py', RUST_BUILD | {'check-python'}),
    (
        'tools/devtools/',
        frozenset({'test-python', 'check-python', 'check-devices', 'build', 'screens', 'harness-virtual'}),
    ),
    ('tools/terrain/', frozenset({'test-python', 'check-python'})),
    ('tools/check-boundaries.py', frozenset({'check-boundaries'})),
    ('tools/pr-gates.sh', frozenset({'test-python'})),
    ('web/', WEB | {'test-python'}),
    ('package.json', WEB),
    ('bun.lock', WEB),
    ('patches/', WEB),
    ('tsconfig.base.json', WEB),
    ('alchemy.run.ts', WEB),
    ('.oxfmtrc.json', WEB),
    ('.oxlintrc.json', WEB),
    ('.oxlintrc.effect.json', WEB),
    ('docs/design/', EVERY),
    ('docs/screenshots/', EVERY),
    ('docs/', NONE),
    # tools/simulator/tests/trace_tools.rs reads this skill.
    ('.agents/skills/ride-trace/', frozenset({'test-rust', 'test-python'})),
    ('.agents/', frozenset({'test-python'})),
    ('.claude/', NONE),
    ('.github/', NONE),
    ('AGENTS.md', NONE),
    ('CLAUDE.md', NONE),
    ('CONTRIBUTING.md', NONE),
    ('README.md', NONE),
    ('LICENSE', NONE),
    ('skills-lock.json', NONE),
)


@dataclass(frozen=True)
class Crate:
    directory: str
    name: str
    path_deps: tuple[str, ...]


def path_deps(manifest, directory, shared):
    found = []
    for table in (manifest, *manifest.get('target', {}).values()):
        for kind in ('dependencies', 'build-dependencies'):
            for name, need in table.get(kind, {}).items():
                if not isinstance(need, dict):
                    continue
                if need.get('workspace'):
                    need, base = shared.get(name), ''
                else:
                    base = directory
                if isinstance(need, dict) and 'path' in need:
                    found.append(os.path.normpath(os.path.join(base, need['path'])))
    return tuple(found)


def members(root):
    workspace = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']
    shared = workspace.get('dependencies', {})
    found = []
    for directory in workspace['members']:
        manifest = tomllib.loads((root / directory / 'Cargo.toml').read_text())
        found.append(Crate(directory, manifest['package']['name'], path_deps(manifest, directory, shared)))
    return found


def firmware_crates(root):
    crates = members(root)
    by_directory = {crate.directory: crate for crate in crates}
    packages = {tomllib.loads(path.read_text())['build']['package'] for path in root.glob('devices/*/device.toml')}
    wanted = [crate.directory for crate in crates if crate.name in packages]
    linked = set()
    while wanted:
        directory = wanted.pop()
        if directory not in linked:
            linked.add(directory)
            if directory in by_directory:
                wanted += by_directory[directory].path_deps
    return linked


def gates_for(path, firmware):
    gates = next(
        (gates for rule, gates in RULES if (path.startswith(rule) if rule.endswith('/') else path == rule)), EVERY
    )
    return gates | {'build'} if any(path.startswith(f'{crate}/') for crate in firmware) else gates


def select(paths, root):
    if not paths:
        return NONE
    firmware = firmware_crates(root)
    return ALWAYS.union(*(gates_for(path, firmware) for path in paths))
