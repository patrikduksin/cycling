import tomllib
from dataclasses import dataclass

from cycling_devtools import workspace


@dataclass(frozen=True)
class Gate:
    name: str
    command: str
    env: tuple[tuple[str, str], ...] = ()
    test: bool = False
    reports: bool = False


BUILD = Gate('build', 'cycling build', (('CYCLING_IMAGE_DIR', '.local/verify/images/build'),))

# Lanes that share a cargo target dir queue on its lock, so clippy gets its own.
LANES = (
    (BUILD,),
    (
        Gate('test-rust', 'cycling test-rust', (('NEXTEST_PROFILE', 'without-ride-scenarios'),), test=True),
        Gate('simulate', 'cycling simulate'),
    ),
    (Gate('test-trouble-host', 'cycling test-trouble-host', test=True),),
    (Gate('test-python', 'cycling test-python', test=True),),
    (Gate('test-web', 'cycling test-web', test=True),),
    (Gate('check-rust', 'cycling check-rust', (('CARGO_TARGET_DIR', 'target/clippy'),)),),
    (
        Gate('check-boundaries', 'cycling check-boundaries'),
        Gate('check-devices', 'cycling check-devices'),
        Gate('check-generated-web', 'cycling check-generated-web'),
        Gate('check-python', 'cycling check-python'),
    ),
    (Gate('check-web', 'cycling check-web'),),
    (Gate('screens', 'cycling sim-compare', reports=True),),
)
ALONE = (Gate('harness-virtual', 'cycling harness-virtual'),)
DEEP = (
    Gate('fuzz', 'nice -n 19 cycling fuzz --fixed'),
    Gate('planted', 'nice -n 19 cycling fuzz-planted'),
)

EVERY = frozenset(gate.name for lane in (*LANES, ALONE) for gate in lane)
NONE = frozenset()
HOST = frozenset(
    {'test-rust', 'simulate', 'check-rust', 'check-boundaries', 'screens', 'harness-virtual', 'test-python'}
)
RUST = HOST | {'build', 'check-devices'}
WEB = frozenset({'test-web', 'check-web'})

# A path no rule matches selects every gate, so a new directory is verified in full until it gets a rule.
RULES = (
    ('apps/', HOST | {'check-generated-web'}),
    ('packages/', HOST),
    ('devices/', RUST),
    ('vendor/', RUST | {'test-trouble-host'}),
    ('Cargo.toml', RUST),
    ('Cargo.lock', RUST),
    ('.cargo/', RUST),
    ('.config/nextest.toml', RUST),
    ('tools/simulator/', HOST),
    ('tools/devtools/src/cycling_devtools/cli.py', EVERY),
    ('tools/devtools/src/cycling_devtools/lanes.py', EVERY),
    ('tools/devtools/src/cycling_devtools/verify.py', EVERY),
    ('tools/devtools/src/cycling_devtools/tree.py', EVERY),
    ('tools/devtools/src/cycling_devtools/admission.py', EVERY),
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


def members(root):
    found = {}
    for member in tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['members']:
        crate = tomllib.loads((root / member / 'Cargo.toml').read_text())
        needs = crate.get('dependencies', {}).items()
        found[member] = (
            crate['package']['name'],
            [need.get('package', name) for name, need in needs if isinstance(need, dict) and 'path' in need],
        )
    return found


def firmware_crates(root):
    crates = {name: (member, needs) for member, (name, needs) in members(root).items()}
    wanted = [tomllib.loads(path.read_text())['build']['package'] for path in root.glob('devices/*/device.toml')]
    linked = set()
    while wanted:
        name = wanted.pop()
        if name in crates and crates[name][0] not in linked:
            linked.add(crates[name][0])
            wanted += crates[name][1]
    return linked


def gates_for(path, firmware):
    gates = next(
        (gates for rule, gates in RULES if (path.startswith(rule) if rule.endswith('/') else path == rule)), EVERY
    )
    return gates | {'build'} if any(path.startswith(f'{crate}/') for crate in firmware) else gates


def select(paths):
    firmware = firmware_crates(workspace.ROOT) if paths else ()
    return frozenset().union(*(gates_for(path, firmware) for path in paths))
