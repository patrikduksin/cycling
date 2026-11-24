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
    assert c606.image_cap == 0x73A000 - 0x2000 - 0x100000 - 0x2000
    assert c606.owned_tail == (0xD96000, 0x104000)


B0 = '{ id = 0, role = "back", edge = "left", at = 250 }'
B1 = '{ id = 1, role = "move", edge = "bottom", at = 217 }'
B2 = '{ id = 2, role = "act", edge = "bottom", at = 783 }'


def test_fitted_rs_holds_the_driver_literals_it_replaced(c606):
    text = devices.fitted_rs(c606)
    assert 'pub const PANEL_WIDTH: usize = 240;' in text
    assert 'pub const PANEL_HEIGHT: usize = 320;' in text
    assert 'pub const PRODUCT_NAME: &str = "VANA C606";' in text
    assert 'pub const FIT_PRODUCT: u16 = 606;' in text
    fixtures = re.findall(
        r'Fixture \{\s*id: (\d+),\s*role: (Some\(Role::\w+\)|None),\s*edge: Side::(\w+),\s*at: (\d+),\s*\}', text
    )
    assert fixtures == [
        ('0', 'Some(Role::Back)', 'Left', '250'),
        ('1', 'Some(Role::Move)', 'Bottom', '217'),
        ('2', 'Some(Role::Act)', 'Bottom', '783'),
    ]
    for fact in ('GPS', 'ANT', 'BLE', 'WIFI', 'SOUND', 'SHUTDOWN', 'CARD'):
        assert f'pub const {fact}: bool = true;' in text
    assert re.search(r'pub const SENSORS: SensorSet = SensorSet \{\s*barometer: true,\s*light: true,\s*\};', text)
    arms = dict(re.findall(r'Region::(\w+) => \((0x[0-9a-f]+, 0x[0-9a-f]+)\)', text))
    assert {region: tuple(int(v, 16) for v in pair.split(', ')) for region, pair in arms.items()} == {
        'Configuration': (0x00E9_8000, 2 * 4096),
        'Data': (0x00D9_8000, 0x10_0000),
        'Crash': (0x0001_E000, 2 * 4096),
        'Intent': (0x0001_C000, 2 * 4096),
        'Screens': (0x00D9_6000, 2 * 4096),
    }


def test_checked_in_output_is_what_the_manifest_generates(c606):
    for path, text in devices.generated(c606).items():
        assert path.read_text() == text, f'{path} is stale: run cycling devices'


def test_models_rs_holds_every_device_and_test_model():
    assert devices.MODELS_RS.read_text() == devices.models_rs(devices.models([devices.parse(C606 / 'device.toml')]))
    found = {model.name: model for model in devices.models([devices.parse(C606 / 'device.toml')])}
    assert found['c606'] == devices.parse(C606 / 'device.toml').model
    bare = found['bare']
    assert (bare.width, bare.height, bare.touch) == (240, 320, False)
    assert [b.role for b in bare.buttons] == ['back', 'move', 'act']
    assert (bare.product, bare.fit_product) == ('VANA BARE', 0)
    assert [part for part, fitted in bare.fitted.items() if not fitted] == ['wifi', 'barometer']


def test_the_volatile_model_backs_its_data_with_memory_that_is_never_durable():
    found = {model.name: model for model in devices.models([devices.parse(C606 / 'device.toml')])}
    volatile = found['volatile']
    assert volatile.memory == (
        devices.Memory('data', 'psram', 0x40000),
        devices.Memory('configuration', 'ram', 0x2000),
    )
    assert not any(r.durable for r in volatile.memory)
    assert volatile.fitted['wifi']
    assert found['c606'].memory == ()
    generated = devices.models_rs(list(found.values()))
    assert 'memory: &[(Region::Data, 0x40000), (Region::Configuration, 0x2000)]' in generated


def test_every_c606_range_is_flash_and_durable(c606):
    assert all(r.durable for r in c606.owned)
    assert re.search(
        r'pub const fn durable\(region: Region\) -> bool \{\s*match region \{(\s*Region::\w+ => true,){4}',
        devices.fitted_rs(c606),
    )


SCREENS = 'screens = { offset = 0xD96000, size = 0x2000, within = "ota_1" }'
# With the data in PSRAM, the screens move up against the configuration range,
# so the owned ranges still close the slot.
SCREENS_ON_TOP = 'screens = { offset = 0xE96000, size = 0x2000, within = "ota_1" }'


def test_a_memory_backed_range_skips_partition_containment_and_the_image_cap(tmp_path):
    memory = 'data = { memory = "psram", size = 0x40000 }'
    device = devices.parse(
        manifest_with(
            tmp_path,
            lambda t: t.replace('data = { offset = 0xD98000, size = 0x100000, within = "ota_1" }', memory).replace(
                SCREENS, SCREENS_ON_TOP
            ),
        )
    )
    assert device.model.memory == (devices.Memory('data', 'psram', 0x40000),)
    assert device.image_cap == 0x73A000 - 0x2000 - 0x2000
    with pytest.raises(devices.ManifestError, match='no driver that backs it yet'):
        devices.fitted_rs(device)


def test_a_role_less_button_generates_none_and_answers_its_bare_id(tmp_path):
    extra = f'  {B2},\n  {{ id = 9, edge = "bottom", at = 500 }},\n'
    device = devices.parse(manifest_with(tmp_path, lambda t: t.replace(f'  {B2},\n', extra)))
    assert re.search(
        r'Fixture \{\s*id: 9,\s*role: None,\s*edge: Side::Bottom,\s*at: 500,\s*\}', devices.fitted_rs(device)
    )
    assert devices.expected_fitted(device)['buttons'] == '0:back,1:move,2:act,9'


VOLATILE_DATA = '\n[owned]\ndata = { memory = "psram", size = 0x40000 }\n'


def model_with(tmp_path, change, name='bare'):
    path = tmp_path / f'{name}.toml'
    path.write_text(change((devices.TEST_MODELS / 'bare.toml').read_text()))
    return path


@pytest.mark.parametrize(
    ('change', 'name', 'rule'),
    [
        (lambda t: t, 'other', 'match its file name'),
        (lambda t: t.replace('[panel]', '[build]\nchip = "esp32s3"\n\n[panel]'), 'bare', 'unknown keys'),
        (lambda t: t.replace(f'  {B2},\n', ''), 'bare', 'at least 3 buttons'),
        (lambda t: t.replace('width = 240', 'width = 170'), 'bare', '3/4 scale'),
        (lambda t: t.replace('name = "bare"', 'name = "Bare"'), 'Bare', 'must match'),
        (lambda t: t.replace('name = "bare"', 'name = "virtual"'), 'virtual', 'reserved'),
        (lambda t: t.replace('light = true\n', ''), 'bare', r'\[fitted\] is missing: light'),
        (lambda t: t.replace('product = "VANA BARE"', 'product = "VANA BARE TOO LONG"'), 'bare', '1 to 15 bytes'),
        (lambda t: t.replace('wifi = true', 'wifi = false') + VOLATILE_DATA, 'bare', 'needs Wi-Fi'),
        (lambda t: t + '\n[owned]\ndata = { offset = 0x0, size = 0x1000, within = "ota_1" }\n', 'bare', 'no flash'),
        (lambda t: t + VOLATILE_DATA.replace('"psram"', '"sram"'), 'bare', 'psram" or "ram'),
        (lambda t: t + VOLATILE_DATA.replace('0x40000', '0x40800'), 'bare', 'whole 0x1000-byte erase sectors'),
        (lambda t: t + '\n[owned]\nspare = { memory = "ram", size = 0x1000 }\n', 'bare', 'unknown keys: spare'),
    ],
)
def test_a_test_model_that_breaks_a_rule_is_refused_naming_it(tmp_path, change, name, rule):
    with pytest.raises(devices.ManifestError, match=rule):
        devices.parse_test_model(model_with(tmp_path, change, name))


def test_the_test_model_cases_start_from_a_model_that_passes(tmp_path):
    assert devices.parse_test_model(model_with(tmp_path, lambda t: t)).name == 'bare'


def test_a_test_model_may_not_take_a_device_name(tmp_path, monkeypatch):
    model_with(tmp_path, lambda t: t.replace('name = "bare"', 'name = "c606"'), 'c606')
    monkeypatch.setattr(devices, 'TEST_MODELS', tmp_path)
    with pytest.raises(devices.ManifestError, match=r'share a name: c606$'):
        devices.models([devices.parse(C606 / 'device.toml')])


def more_buttons(count):
    return ''.join(f'  {{ id = {10 + i}, edge = "top", at = 500 }},\n' for i in range(count))


def test_buttons_may_list_any_ids_in_any_order_up_to_eight(tmp_path):
    listed = ''.join(
        f'  {{ id = {id}, role = "{role}", edge = "left", at = {at} }},\n'
        for id, role, at in ((200, 'act', 900), (7, 'back', 0), (3, 'move', 333))
    ).replace('"move", edge = "left"', '"move", edge = "right"')
    path = manifest_with(
        tmp_path,
        lambda t: t.replace(
            f'  {B0},\n  {B1},\n  {B2},\n',
            listed + more_buttons(5),
        ),
    )
    assert [b.id for b in devices.parse(path).model.buttons] == [200, 7, 3, 10, 11, 12, 13, 14]


@pytest.mark.parametrize('at', [0, 333, 500, 1000])
def test_a_button_with_a_role_may_sit_anywhere_along_its_edge(tmp_path, at):
    path = manifest_with(tmp_path, lambda t: t.replace(B0, B0.replace('at = 250', f'at = {at}')))
    assert devices.parse(path).model.buttons[0].at == at


def manifest_with(tmp_path, change):
    root = tmp_path / 'c606'
    root.mkdir()
    shutil.copy(C606 / 'Cargo.toml', root / 'Cargo.toml')
    (root / 'device.toml').write_text(change((C606 / 'device.toml').read_text()))
    return root / 'device.toml'


@pytest.mark.parametrize(
    ('change', 'rule'),
    [
        (lambda t: t.replace(f'  {B2},\n', ''), 'at least 3 buttons'),
        (lambda t: t.replace(B2, B2.replace('id = 2', 'id = 1')), 'share an id'),
        (lambda t: t.replace('role = "act"', 'role = "back"'), 'exactly one button'),
        (lambda t: t.replace(B2, B2.replace(' role = "act",', '')), 'exactly one button'),
        (lambda t: t.replace('{ id = 2,', '{ id = 256,'), 'integers from 0 to 255'),
        (lambda t: t.replace('{ id = 2,', '{ id = -1,'), 'integers from 0 to 255'),
        (
            lambda t: t.replace('edge = "bottom"', 'edge = "front"'),
            'edge .front. must be one of left, right, top, bottom',
        ),
        (lambda t: t.replace('at = 250', 'at = 1001'), 'at 1001 must be an integer from 0 to 1000'),
        (lambda t: t.replace('at = 250', 'at = 0.25'), 'at 0.25 must be an integer'),
        (lambda t: t.replace(', at = 250', ''), r'button 0 is missing: at'),
        (
            lambda t: t.replace(f'  {B2},\n', f'  {B2},\n' + more_buttons(6)),
            'at most 8 buttons',
        ),
        (lambda t: t.replace('"VANA C606"', '"VANA C606 EXTRA L"'), '1 to 15 bytes'),
        (lambda t: t.replace('"VANA C606"', '""'), '1 to 15 bytes'),
        (lambda t: t.replace('"VANA C606"', '"VANA Ç606"'), 'printable ASCII'),
        (lambda t: t.replace('fit_product = 606', 'fit_product = 65536'), 'fit_product 65536 must be an integer'),
        (lambda t: t.replace('fit_product = 606', 'fit_product = -1'), 'fit_product -1 must be an integer'),
        (lambda t: t.replace('fit_product = 606\n', ''), r'\[device\] is missing: fit_product'),
        (lambda t: t.replace('product = "VANA C606"\n', ''), r'\[device\] is missing: product'),
        (lambda t: t.replace('shutdown = true\n', ''), r'\[fitted\] is missing: shutdown'),
        (lambda t: t.replace('card = true\n', 'card = true\nradio = true\n'), r'\[fitted\] has unknown keys: radio'),
        (lambda t: t.replace('gps = true', 'gps = "yes"'), r'\[fitted\] gps must be true or false'),
        (lambda t: t[: t.index('\n[fitted]')] + '\n', 'the manifest is missing: fitted'),
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
                .replace('0xD96000', '0x18000')
                .replace('0xE98000', '0x1A000')
                .replace('size = 0x100000', 'size = 0x8000')
            ),
            'exactly one application slot',
        ),
        (lambda t: t.replace('size = "16MB"', 'size = "16384KB"'), 'whole number of MB'),
        (
            lambda t: (
                t.replace(
                    'data = { offset = 0xD98000, size = 0x100000, within = "ota_1" }',
                    'data = { memory = "psram", size = 0x40000 }',
                )
                .replace(SCREENS, SCREENS_ON_TOP)
                .replace('wifi = true', 'wifi = false')
            ),
            'needs Wi-Fi',
        ),
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
    facts = {'touch': 'ready', 'shutdown': 'supported', 'barometer': 'fitted', 'light': 'fitted'}
    return facts | {key: str(value) for key, value in fields.items()} | changes


def test_a_unit_that_answers_its_manifest_matches(c606):
    assert devices.compare_fitted(c606, unit_reply(c606, width='240', touch='failed')) == []


def test_each_disagreement_with_the_unit_is_named(c606):
    reply = unit_reply(
        c606,
        data_offset='0xd99000',
        buttons='0:back,1:move',
        touch='unsupported',
        shutdown='unsupported',
        barometer='absent',
        light='absent',
    )
    assert devices.compare_fitted(c606, reply) == [
        "buttons: unit '0:back,1:move', manifest '0:back,1:move,2:act'",
        "data_offset: unit '0xd99000', manifest 14254080",
        "touch: unit 'unsupported', manifest fitted=True",
        "shutdown: unit 'unsupported', manifest fitted=True",
        "barometer: unit 'absent', manifest fitted=True",
        "light: unit 'absent', manifest fitted=True",
    ]


def test_a_unit_reporting_more_than_its_manifest_fits_is_named(tmp_path):
    absent = 'shutdown = false\ncard = true\nbarometer = false\nlight = false\n'
    path = manifest_with(
        tmp_path, lambda t: t.replace('shutdown = true\ncard = true\nbarometer = true\nlight = true\n', absent)
    )
    device = devices.parse(path)
    assert (
        devices.compare_fitted(device, unit_reply(device, shutdown='unsupported', barometer='absent', light='absent'))
        == []
    )
    assert devices.compare_fitted(device, unit_reply(device)) == [
        "shutdown: unit 'supported', manifest fitted=False",
        "barometer: unit 'fitted', manifest fitted=False",
        "light: unit 'fitted', manifest fitted=False",
    ]


def test_an_unknown_shutdown_is_a_note_not_a_mismatch(c606, capsys):
    assert devices.compare_fitted(c606, unit_reply(c606, shutdown='unknown')) == []
    assert 'shutdown=unknown' in capsys.readouterr().out


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
