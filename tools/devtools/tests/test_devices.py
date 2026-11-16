import re
import shlex
import shutil
import tomllib

import pytest

from cycling_devtools import cli, devices
from cycling_devtools.workspace import ROOT

C606 = ROOT / 'devices' / 'c606'


@pytest.fixture
def c606():
    return devices.parse(C606 / 'device.toml')


def test_build_facts_equal_the_literals_build_sh_held(c606):
    env = dict(line.split('=', 1) for line in devices.build_env(c606, None).splitlines())
    assert {key: shlex.split(value)[0] for key, value in env.items() if value} | {'variant': ''} == {
        'device': 'c606',
        'device_dir': 'devices/c606',
        'package': 'c606-firmware',
        'bin': 'cycling-os',
        'target': 'xtensa-esp32s3-none-elf',
        'chip': 'esp32s3',
        'features': 'hardware',
        'variant': '',
        'flash_size': '16mb',
        'flash_mode': 'dio',
        'flash_freq': '80mhz',
        'app_partition': 'ota_1',
        'stack_budget': '61440',
        'nm': 'xtensa-esp32s3-elf-nm',
        'stack_start_symbol': '_stack_start_cpu0',
        'stack_end_symbol': '_stack_end_cpu0',
    }
    assert c606.features('maintenance') == 'hardware,bulk-maintenance'
    assert c606.features('scripted-gnss') == 'hardware,scripted-gnss'
    with pytest.raises(devices.ManifestError, match='no build variant'):
        c606.features('gnss')


def test_partitions_equal_the_table_and_layout_firmware_py_held(c606):
    assert [(p.name, *p.entry) for p in c606.partitions] == [
        ('nvs', 1, 2, 0x9000, 0x4000),
        ('otadata', 1, 0, 0xD000, 0x2000),
        ('phy_init', 1, 1, 0xF000, 0x1000),
        ('coredump', 1, 3, 0x10000, 0x10000),
        ('ota_0', 0, 0x10, 0x20000, 0x73A000),
        ('ota_1', 0, 0x11, 0x760000, 0x73A000),
    ]
    assert c606.partition_csv().splitlines()[1:] == [
        'nvs,data,nvs,0x9000,0x4000,',
        'otadata,data,ota,0xd000,0x2000,',
        'phy_init,data,phy,0xf000,0x1000,',
        'coredump,data,coredump,0x10000,0x10000,',
        'ota_0,app,ota_0,0x20000,0x73a000,',
        'ota_1,app,ota_1,0x760000,0x73a000,',
    ]
    assert c606.flash_size == 0x1000000
    assert c606.image_cap == 0x73A000 - 0x2000 - 0x100000
    assert c606.owned_tail == (0xD98000, 0x102000)


def test_fitted_rs_holds_the_driver_literals_it_replaced(c606):
    text = devices.fitted_rs(c606)
    assert 'pub const PANEL_WIDTH: usize = 240;' in text
    assert 'pub const PANEL_HEIGHT: usize = 320;' in text
    assert 'pub const BUTTONS: &[Button] = &[Button::TopLeft, Button::BottomLeft, Button::BottomRight];' in text
    arms = dict(re.findall(r'Region::(\w+) => \((0x[0-9a-f]+, 0x[0-9a-f]+)\)', text))
    assert {region: tuple(int(v, 16) for v in pair.split(', ')) for region, pair in arms.items()} == {
        'Configuration': (0x00E9_8000, 2 * 4096),
        'Data': (0x00D9_8000, 0x10_0000),
        'Crash': (0x0001_E000, 2 * 4096),
    }


def test_checked_in_output_is_what_the_manifest_generates(c606):
    for path, text in devices.generated(c606).items():
        assert path.read_text() == text, f'{path} is stale: run cycling devices'


def manifest_with(tmp_path, change):
    root = tmp_path / 'c606'
    root.mkdir()
    shutil.copy(C606 / 'Cargo.toml', root / 'Cargo.toml')
    (root / 'device.toml').write_text(change((C606 / 'device.toml').read_text()))
    return root / 'device.toml'


@pytest.mark.parametrize(
    ('change', 'rule'),
    [
        (lambda t: t.replace('  { id = 2, role = "act" },\n', ''), 'at least 3 buttons'),
        (lambda t: t.replace('{ id = 2, role = "act" }', '{ id = 1, role = "act" }'), 'share an id'),
        (lambda t: t.replace('role = "act"', 'role = "back"'), 'exactly one button'),
        (lambda t: t.replace('{ id = 2, role = "act" }', '{ id = 2 }'), 'exactly one button'),
        (lambda t: t.replace('{ id = 2,', '{ id = 7,'), 'button ids run'),
        (lambda t: t.replace('offset = 0x1E000, size = 0x2000', 'offset = 0x1E800, size = 0x1000'), 'erase sector'),
        (lambda t: t.replace('offset = 0xD98000, size = 0x100000', 'offset = 0xD98000, size = 0x101000'), 'overlap'),
        (lambda t: t.replace('offset = 0x1E000, size = 0x2000', 'offset = 0x1F000, size = 0x2000'), 'inside'),
        (lambda t: t.replace('within = "coredump"', 'within = "nvsx"'), 'unknown partition'),
        (lambda t: t.replace('size = 0x2000, within = "ota_1"', 'size = 0x1000, within = "ota_1"'), 'close the slot'),
        (lambda t: t.replace('width = 240', 'width = 170'), '3/4 scale'),
        (lambda t: t.replace('height = 320', 'height = 600'), 'outside'),
        (lambda t: t.replace('chip = "esp32s3"', 'chip = "esp32c6"'), 'esp-hal feature'),
        (lambda t: t.replace('package = "c606-firmware"', 'package = "c607-firmware"'), 'not the crate'),
        (lambda t: t.replace('"scripted-gnss" }', '"no-such-feature" }'), 'not declared'),
        (lambda t: t.replace('name = "c606"', 'name = "c607"'), 'match its directory'),
        (lambda t: t.replace('[panel]', 'colour = "red"\n\n[panel]'), 'unknown keys'),
        (lambda t: t.replace('size = 0x73A000 },\n]', 'size = 0x739000 },\n]'), 'equal size'),
        (lambda t: t.replace('offset = 0xD000, size = 0x2000', 'offset = 0xD000, size = 0x1000'), 'two sectors'),
        (lambda t: t.replace('subtype = "ota_1"', 'subtype = "ota_0"'), 'ota_0 then ota_1'),
        (
            lambda t: (
                t.replace('within = "ota_1"', 'within = "coredump"')
                .replace('0xD98000', '0x10000')
                .replace('0xE98000', '0x1A000')
                .replace('size = 0x100000', 'size = 0x8000')
            ),
            'exactly one application slot',
        ),
        (lambda t: t.replace('size = "16MB"', 'size = "16384KB"'), 'whole number of MB'),
        (
            lambda t: t.replace(
                'data = { offset = 0xD98000, size = 0x100000, within = "ota_1" }',
                'data = { offset = 0x65A000, size = 0x100000, within = "ota_0" }',
            ),
            'exactly one application slot',
        ),
    ],
)
def test_a_manifest_that_breaks_a_rule_is_refused_naming_it(tmp_path, change, rule):
    with pytest.raises(devices.ManifestError, match=rule):
        devices.parse(manifest_with(tmp_path, change))


def test_the_rule_cases_start_from_a_manifest_that_passes(tmp_path):
    assert devices.parse(manifest_with(tmp_path, lambda t: t)).name == 'c606'


def unit_reply(c606, **changes):
    fields = {
        key: hex(value) if isinstance(value, int) else value for key, value in devices.expected_fitted(c606).items()
    }
    return {'touch': 'ready'} | {key: str(value) for key, value in fields.items()} | changes


def test_a_unit_that_answers_its_manifest_matches(c606):
    assert devices.compare_fitted(c606, unit_reply(c606, width='240', touch='failed')) == []


def test_each_disagreement_with_the_unit_is_named(c606):
    reply = unit_reply(c606, data_offset='0xd99000', buttons='0,1', touch='unsupported')
    assert devices.compare_fitted(c606, reply) == [
        "buttons: unit '0,1', manifest '0,1,2'",
        "data_offset: unit '0xd99000', manifest 14254080",
        "touch: unit 'unsupported', manifest fitted=True",
    ]


def test_another_device_is_refused_before_any_field(c606):
    assert devices.compare_fitted(c606, unit_reply(c606, device='c607', width='1')) == [
        "wrong device: the unit is 'c607', the manifest is 'c606'"
    ]


def test_device_tasks_take_device_into_the_environment():
    assert cli.take_device(['--device', 'c606', '--variant', 'maintenance']) == (['--variant', 'maintenance'], 'c606')
    assert cli.take_device(['--device=c606']) == ([], 'c606')
    assert cli.take_device(['x']) == (['x'], None)
    with pytest.raises(ValueError, match='needs a name'):
        cli.take_device(['--device'])
    assert {t.name for t in cli.TASKS if t.takes_device} >= {'build', 'flash', 'backup', 'stock', 'devices'}


def test_every_manifest_names_a_crate_cargo_knows():
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['members']
    for path in devices.manifests():
        assert str(path.parent.relative_to(ROOT)) in workspace


def test_the_crash_record_names_each_variant_as_its_archived_elf(c606):
    crash = (ROOT / 'packages' / 'device-api' / 'src' / 'crash.rs').read_text()
    for variant in c606.variants:
        assert f'"-{variant}"' in crash and f'"-dirty-{variant}"' in crash, variant


def test_a_build_refuses_generated_files_the_manifest_no_longer_matches(tmp_path, capsys):
    path = manifest_with(tmp_path, lambda t: t)
    device = devices.parse(path)
    (device.root / 'src').mkdir()
    (device.root / 'src' / 'fitted.rs').write_text(devices.fitted_rs(device).replace('0xd98000', '0xd99000'))
    assert devices.stale(device) == [device.root / 'src' / 'fitted.rs']
    (device.root / 'src' / 'fitted.rs').write_text(devices.fitted_rs(device))
    assert devices.stale(device) == []
