"""A replay leaves the recorded world, and reports VANA's panics, by unwinding, so every profile a host tool builds
with must unwind. Only the firmware's profile aborts, which its no_std target needs."""

import tomllib

from cycling_devtools.twin import Tool
from cycling_devtools.workspace import ROOT

PROFILES = tomllib.loads((ROOT / 'Cargo.toml').read_text())['profile']
BUILT_IN = {'dev': None, 'release': None, 'test': 'dev', 'bench': 'release'}


def strategy(name):
    profile = PROFILES.get(name, {})
    if 'panic' in profile:
        return profile['panic']
    parent = profile.get('inherits', BUILT_IN.get(name))
    return 'unwind' if parent is None else strategy(parent)


def test_every_profile_a_host_tool_builds_with_unwinds():
    for name in {'dev', 'release', 'test', 'bench', *PROFILES} - {'firmware'}:
        assert strategy(name) == 'unwind', f'[profile.{name}] aborts, so a replay that leaves the recorded world dies'


def test_the_firmware_builds_with_the_one_profile_that_aborts():
    assert strategy('firmware') == 'abort'
    assert '--profile firmware' in (ROOT / 'devices/build.sh').read_text()


def test_the_trace_tool_ships_in_a_profile_that_unwinds():
    shipped = (ROOT / 'tools/simulator/trace.sh').read_text().split('--profile', 1)[1].split()[0]
    assert strategy(shipped) == 'unwind', f'trace.sh ships the trace tool in [profile.{shipped}], which aborts'
    assert strategy(Tool().profile) == 'unwind', f'twin.py builds in [profile.{Tool().profile}], which aborts'
