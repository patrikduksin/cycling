"""The gates of `cycling verify`, the lanes they run in, and which changed paths select them."""

import tomllib
from dataclasses import dataclass

from cycling_devtools import workspace


@dataclass(frozen=True)
class Gate:
    name: str
    command: str
    env: tuple[tuple[str, str], ...] = ()
    # The review loop starts its reviewers once every gate marked test has passed.
    test: bool = False
    # Takes --pr and --video, and its result depends on the base it compares with.
    reports: bool = False


# Its image stays apart from .local/cycling.bin, which flash writes.
BUILD = Gate('build', 'cycling build', (('CYCLING_IMAGE_DIR', '.local/verify/images/build'),))

# Lanes run side by side, each one's gates in order. Lanes that share a cargo
# target dir would only queue on its lock, so clippy gets its own and the
# firmware has one; sccache shares the dependency builds between them. The
# firmware link is the floor of every run, so its lane starts first.
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
# harness-virtual has real-time reply timeouts, so it runs after the loaded phase.
ALONE = (Gate('harness-virtual', 'cycling harness-virtual'),)
# Tier 2, `cycling verify --deep`: one lane that no changed path selects.
DEEP = (
    Gate('fuzz', 'nice -n 19 cycling fuzz --fixed'),
    Gate('planted', 'nice -n 19 cycling fuzz-planted'),
)

EVERY = frozenset(gate.name for lane in (*LANES, ALONE) for gate in lane)
NONE = frozenset()
# Host Rust reaches pytest through ride-fit, which the export tests build and run.
HOST = frozenset(
    {'test-rust', 'simulate', 'check-rust', 'check-boundaries', 'screens', 'harness-virtual', 'test-python'}
)
RUST = HOST | {'build', 'check-devices'}
WEB = frozenset({'test-web', 'check-web'})

# The first rule that matches a changed path names its gates: a trailing slash
# matches everything below, anything else the exact path. A path no rule
# matches selects every gate, so a new directory is verified in full until it
# gets a rule. A workspace crate adds the firmware build when a device links it
# (firmware_crates).
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
    # The modules that define and run the gates.
    ('tools/devtools/src/cycling_devtools/cli.py', EVERY),
    ('tools/devtools/src/cycling_devtools/lanes.py', EVERY),
    ('tools/devtools/src/cycling_devtools/verify.py', EVERY),
    ('tools/devtools/src/cycling_devtools/tree.py', EVERY),
    ('tools/devtools/src/cycling_devtools/admission.py', EVERY),
    # The firmware build stamps its image and reads its stack frames with devtools.
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
    # Scenario tests include the design mockups, and the README screenshots are rendered from checkpoints.
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
    """Each workspace crate's directory, with its package name and its path dependencies' names."""
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
    """Directories of the crates a firmware links: the package each devices/*/device.toml builds, and what it needs."""
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
    """Names of the gates that the changed paths select."""
    firmware = firmware_crates(workspace.ROOT) if paths else ()
    return frozenset().union(*(gates_for(path, firmware) for path in paths))
