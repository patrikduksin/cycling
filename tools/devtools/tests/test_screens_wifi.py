import json
from pathlib import Path

from cycling_devtools.connectivity import screens

# Written by tools/simulator/tests/screens_api.rs from a booted simulator.
LISTED = json.loads((Path(__file__).parent / 'fixtures/screens_api.json').read_text())


def nodes(listed):
    found = bytes.fromhex(screens.main_page(listed)['nodes'])
    return [found[at : at + 4] for at in range(0, len(found), 4)]


def test_the_cadence_field_shows_power_and_nothing_else_changes():
    catalogue = LISTED['catalogue']
    field, cadence = screens.tile(catalogue, 'field', 'CAD RPM')
    _, power = screens.tile(catalogue, 'field', 'PWR 3S W')
    leaf = catalogue['kinds']['leaf']
    before = nodes(LISTED['screens'])
    after = nodes({'screens': [{'name': 'ride/main', 'nodes': screens.edited(LISTED['screens'], catalogue).hex()}]})

    changed = [at for at, (old, new) in enumerate(zip(before, after, strict=True)) if old != new]
    assert len(before) == len(after) and len(changed) == 1
    old, new = before[changed[0]], after[changed[0]]
    assert old[0] == new[0] == leaf + field
    assert (old[1], new[1]) == (cadence, power)
    assert old[2:] == new[2:]


def test_a_hero_cadence_is_found_through_its_hero_bit():
    catalogue = LISTED['catalogue']
    field, cadence = screens.tile(catalogue, 'field', 'CAD RPM')
    _, power = screens.tile(catalogue, 'field', 'PWR 3S W')
    hero = catalogue['kinds']['hero'] | catalogue['kinds']['leaf'] + field
    listed = {'screens': [{'name': 'ride/main', 'nodes': bytes([hero, cadence, 0x80, 0]).hex()}]}
    assert screens.edited(listed, catalogue) == bytes([hero, power, 0x80, 0])
