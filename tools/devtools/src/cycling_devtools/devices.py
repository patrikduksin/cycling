"""Device manifests: devices/<name>/device.toml declares a model once, and everything else derives from it.

    cycling devices                 # check every manifest; write each devices/<name>/src/fitted.rs, VANA's limits.rs and the simulator's models.rs
    cycling devices --locked        # fail, naming each stale file, instead of writing
    cycling devices --check         # compare the attached unit's FITTED reply with its manifest

build.sh, flash, backup and the other device tasks read the manifest that
`--device <name>` (CYCLING_DEVICE) selects; it may be left out while exactly one exists.

Test-only models in tools/simulator/models/<name>.toml declare only [device], [panel],
[controls], [fitted] and, optionally, memory-backed [owned] ranges. They feed
tools/simulator/src/models.rs and nothing that builds firmware.

An [owned] range is flash-backed (offset, size, within), which survives power-off, or
memory-backed (memory = "psram" or "ram", size), which never does.
"""

import argparse
import itertools
import json
import os
import re
import shlex
import subprocess
import sys
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools.device.port import select_port
from cycling_devtools.terminal.usb import UsbConnection, reply_fields
from cycling_devtools.workspace import ROOT

DEVICES = ROOT / 'devices'
TEST_MODELS = ROOT / 'tools' / 'simulator' / 'models'
MODELS_RS = ROOT / 'tools' / 'simulator' / 'src' / 'models.rs'
LIMITS_RS = ROOT / 'apps' / 'vana' / 'src' / 'screens' / 'limits.rs'
ERASE_SECTOR = 0x1000
HARDWARE_FEATURE = 'hardware'
REGIONS = ('configuration', 'data', 'crash', 'intent', 'screens')
MEMORIES = ('psram', 'ram')
ROLES = ('act', 'back', 'move')
MIN_BUTTONS = 3
MAX_BUTTONS = 8
MAX_BUTTON_ID = 255
SIDES = ('left', 'right', 'top', 'bottom')
ALONG = 1000
MIDDLE = range(ALONG // 3 + 1, ALONG - ALONG // 3)
MAX_PRODUCT_BYTES = 15
MAX_FIT_PRODUCT = 0xFFFF
FITTED = ('gps', 'ant', 'ble', 'wifi', 'sound', 'shutdown', 'card', 'barometer', 'light')
SENSORS = ('barometer', 'light')
NAME = r'[a-z][a-z0-9-]{0,15}'
RESERVED_NAMES = ('virtual',)
BASE_PANEL = (240, 320)
MIN_SCALE_QUARTERS = 3
MAX_PANEL = (320, 512)
PARTITION_TYPES = {
    'app': (0x00, {'factory': 0x00, 'ota_0': 0x10, 'ota_1': 0x11}),
    'data': (0x01, {'ota': 0x00, 'phy': 0x01, 'nvs': 0x02, 'coredump': 0x03}),
}
ARCHITECTURES = {
    'xtensa-esp32s3-none-elf': {
        'nm': 'xtensa-esp32s3-elf-nm',
        'stack_start': '_stack_start_cpu0',
        'stack_end': '_stack_end_cpu0',
    },
}


class ManifestError(ValueError):
    pass


@dataclass(frozen=True)
class Partition:
    name: str
    type: str
    subtype: str
    offset: int
    size: int

    @property
    def end(self):
        return self.offset + self.size

    @property
    def entry(self):
        kind, subtypes = PARTITION_TYPES[self.type]
        return kind, subtypes[self.subtype], self.offset, self.size


@dataclass(frozen=True)
class Owned:
    region: str
    offset: int
    size: int
    within: str
    durable = True

    @property
    def end(self):
        return self.offset + self.size


@dataclass(frozen=True)
class Memory:
    region: str
    memory: str
    size: int
    durable = False


@dataclass(frozen=True)
class Fixture:
    id: int
    edge: str
    at: int
    role: str | None = None


@dataclass(frozen=True)
class Model:
    name: str
    product: str
    fit_product: int
    width: int
    height: int
    touch: bool
    buttons: tuple[Fixture, ...]
    fitted: dict[str, bool]
    memory: tuple[Memory, ...] = ()


@dataclass(frozen=True)
class Device:
    root: Path
    chip: str
    target: str
    package: str
    bin: str
    flash_size: int
    flash_mode: str
    flash_freq: str
    app_partition: str
    stack_budget: int
    variants: dict[str, str]
    partition_table: int
    partitions: tuple[Partition, ...]
    owned: tuple[Owned | Memory, ...]
    model: Model

    @property
    def name(self):
        return self.model.name

    @property
    def flash(self):
        return tuple(r for r in self.owned if isinstance(r, Owned))

    def partition(self, name):
        return next(p for p in self.partitions if p.name == name)

    @property
    def slots(self):
        return tuple(p for p in self.partitions if p.type == 'app')

    @property
    def image_cap(self):
        room = [p.size for p in self.slots]
        for slot in self.slots:
            room += [r.offset - slot.offset for r in self.flash if r.within == slot.name]
        return min(room)

    @property
    def owned_tail(self):
        inside = [r for r in self.flash if r.within in {p.name for p in self.slots}]
        start = min(r.offset for r in inside)
        return start, max(r.end for r in inside) - start

    def features(self, variant=None):
        if variant is None:
            return HARDWARE_FEATURE
        if variant not in self.variants:
            known = ', '.join(sorted(self.variants)) or 'none'
            raise ManifestError(f'{self.name} has no build variant {variant!r} (variants: {known})')
        return f'{HARDWARE_FEATURE},{self.variants[variant]}'

    def partition_csv(self):
        rows = [f'{p.name},{p.type},{p.subtype},{p.offset:#x},{p.size:#x},' for p in self.partitions]
        return (
            f'# Generated from devices/{self.name}/device.toml by build.sh. Never flash this table.\n'
            + '\n'.join(rows)
            + '\n'
        )


def table(value, where, keys, optional=()):
    if not isinstance(value, dict):
        raise ManifestError(f'{where} must be a table')
    unknown = set(value) - set(keys) - set(optional)
    if unknown:
        raise ManifestError(f'{where} has unknown keys: {", ".join(sorted(unknown))}')
    missing = set(keys) - set(value)
    if missing:
        raise ManifestError(f'{where} is missing: {", ".join(sorted(missing))}')
    return value


def size_bytes(text):
    if text.endswith('MB') and text[:-2].isdigit():
        return int(text[:-2]) << 20
    raise ManifestError(f'flash size {text!r} must be a whole number of MB, such as "16MB"')


def parse(path):
    try:
        raw = tomllib.loads(path.read_text())
    except tomllib.TOMLDecodeError as error:
        raise ManifestError(f'{path}: {error}') from None
    try:
        device = build(raw, path.parent)
        check(device, path.parent)
    except ManifestError as error:
        raise ManifestError(f'{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {error}') from None
    return device


def build(raw, root):
    table(raw, 'the manifest', ('device', 'build', 'partitions', 'owned', 'panel', 'controls', 'fitted'))
    spec = table(
        raw['build'],
        '[build]',
        ('chip', 'target', 'package', 'bin', 'flash', 'app_partition', 'stack_budget'),
        optional=('variants',),
    )
    flash = table(spec['flash'], '[build].flash', ('size', 'mode', 'freq'))
    layout = table(raw['partitions'], '[partitions]', ('table', 'rows'))
    rows = layout['rows']
    partitions = tuple(
        Partition(**table(row, f'[partitions] row {index}', ('name', 'type', 'subtype', 'offset', 'size')))
        for index, row in enumerate(rows)
    )
    owned = tuple(owned_range(region, value) for region, value in table(raw['owned'], '[owned]', REGIONS).items())
    model = build_model(raw, tuple(r for r in owned if isinstance(r, Memory)))
    return Device(
        root=root,
        chip=spec['chip'],
        target=spec['target'],
        package=spec['package'],
        bin=spec['bin'],
        flash_size=size_bytes(flash['size']),
        flash_mode=flash['mode'],
        flash_freq=flash['freq'],
        app_partition=spec['app_partition'],
        stack_budget=spec['stack_budget'],
        variants=dict(spec.get('variants', {})),
        partition_table=layout['table'],
        partitions=partitions,
        owned=owned,
        model=model,
    )


def owned_range(region, value):
    where = f'[owned].{region}'
    if isinstance(value, dict) and 'memory' in value:
        return Memory(region, **table(value, where, ('memory', 'size')))
    return Owned(region, **table(value, where, ('offset', 'size', 'within')))


def build_model(raw, memory=()):
    identity = table(raw['device'], '[device]', ('name', 'product', 'fit_product'))
    panel = table(raw['panel'], '[panel]', ('width', 'height'))
    controls = table(raw['controls'], '[controls]', ('touch', 'buttons'))
    buttons = tuple(
        Fixture(**table(button, f'[controls] button {index}', ('id', 'edge', 'at'), optional=('role',)))
        for index, button in enumerate(controls['buttons'])
    )
    fitted = table(raw['fitted'], '[fitted]', FITTED)
    return Model(
        identity['name'],
        identity['product'],
        identity['fit_product'],
        panel['width'],
        panel['height'],
        controls['touch'],
        buttons,
        {fact: fitted[fact] for fact in FITTED},
        memory,
    )


def parse_test_model(path):
    try:
        raw = tomllib.loads(path.read_text())
        table(raw, 'the model', ('device', 'panel', 'controls', 'fitted'), optional=('owned',))
        owned = table(raw.get('owned', {}), '[owned]', (), optional=REGIONS)
        ranges = tuple(owned_range(region, value) for region, value in owned.items())
        if flash := [r.region for r in ranges if not isinstance(r, Memory)]:
            raise ManifestError(f'owned range {flash[0]} must be memory-backed: a test model has no flash')
        model = build_model(raw, ranges)
        if model.name != path.stem:
            raise ManifestError(f'[device].name {model.name!r} must match its file name {path.stem!r}')
        check_model(model)
    except (tomllib.TOMLDecodeError, ManifestError) as error:
        raise ManifestError(f'{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {error}') from None
    return model


def check(device, root):
    if device.name != root.name:
        raise ManifestError(f'[device].name {device.name!r} must match its directory {root.name!r}')
    check_cargo(device, root / 'Cargo.toml')
    check_partitions(device)
    check_owned(device)
    check_model(device.model)


def check_model(model):
    check_name(model)
    check_product(model)
    check_panel(model)
    check_buttons(model)
    check_fitted(model)
    check_memory(model)


def check_name(model):
    if not re.fullmatch(NAME, model.name):
        raise ManifestError(f'[device].name {model.name!r} must match {NAME}')
    if model.name in RESERVED_NAMES:
        raise ManifestError(f"[device].name {model.name!r} is reserved for the simulator's sized shell sessions")


def check_product(model):
    product = model.product
    if not (isinstance(product, str) and product.isascii() and product.isprintable()):
        raise ManifestError(f'[device].product {product!r} must be printable ASCII: the FIT product name is ASCII')
    if not 1 <= len(product) <= MAX_PRODUCT_BYTES:
        raise ManifestError(
            f'[device].product {product!r} must be 1 to {MAX_PRODUCT_BYTES} bytes: '
            'the FIT product name is 16 bytes with its terminator'
        )
    fit = model.fit_product
    if type(fit) is not int or not 0 <= fit <= MAX_FIT_PRODUCT:
        raise ManifestError(f'[device].fit_product {fit!r} must be an integer from 0 to {MAX_FIT_PRODUCT}')


def check_fitted(model):
    if wrong := [fact for fact, value in model.fitted.items() if not isinstance(value, bool)]:
        raise ManifestError(f'[fitted] {", ".join(wrong)} must be true or false')


def check_memory(model):
    for r in model.memory:
        if r.memory not in MEMORIES:
            raise ManifestError(f'owned range {r.region} must name memory "psram" or "ram", not {r.memory!r}')
        if type(r.size) is not int or r.size <= 0 or not aligned(r.size):
            raise ManifestError(
                f'owned range {r.region} must be a nonzero run of whole {ERASE_SECTOR:#x}-byte erase sectors'
            )
    if any(r.region == 'data' for r in model.memory) and not model.fitted['wifi']:
        raise ManifestError('a memory-backed data range needs Wi-Fi: only the phone can keep its rides')


def check_cargo(device, path):
    """Cargo stays the authority for the chip: HAL crate features choose it."""
    cargo = tomllib.loads(path.read_text())
    if cargo['package']['name'] != device.package:
        raise ManifestError(f'[build].package {device.package!r} is not the crate in {path.name}')
    if device.bin not in {b['name'] for b in cargo.get('bin', [])}:
        raise ManifestError(f'[build].bin {device.bin!r} is not a [[bin]] of {device.package}')
    features = cargo.get('features', {})
    for feature in (HARDWARE_FEATURE, *device.variants.values()):
        if feature not in features:
            raise ManifestError(f'feature {feature!r} is not declared in {path.name}')
    if 'dirty' in device.variants:
        raise ManifestError('a variant may not be named "dirty": archived ELF names use -dirty for unclean builds')
    hal = cargo.get('dependencies', {}).get('esp-hal', {})
    if device.chip not in hal.get('features', []):
        raise ManifestError(f'[build].chip {device.chip!r} is not an esp-hal feature in {path.name}')
    if device.chip not in device.target.split('-'):
        raise ManifestError(f'[build].target {device.target!r} does not name the chip {device.chip!r}')
    if device.target not in ARCHITECTURES:
        raise ManifestError(f'[build].target {device.target!r} has no known stack symbols or binutils')


def aligned(*values):
    return all(value % ERASE_SECTOR == 0 for value in values)


def check_partitions(device):
    previous_end = device.partition_table + ERASE_SECTOR
    names = set()
    for p in device.partitions:
        if p.type not in PARTITION_TYPES or p.subtype not in PARTITION_TYPES[p.type][1]:
            raise ManifestError(f'partition {p.name} has unknown type {p.type}/{p.subtype}')
        if p.name in names:
            raise ManifestError(f'partition {p.name} is declared twice')
        names.add(p.name)
        if not aligned(p.offset, p.size) or p.size == 0:
            raise ManifestError(f'partition {p.name} must be a nonzero run of whole {ERASE_SECTOR:#x}-byte sectors')
        if p.offset < previous_end:
            raise ManifestError(f'partition {p.name} overlaps the one before it or the partition table')
        previous_end = p.end
    if previous_end > device.flash_size:
        raise ManifestError('the partitions extend past the flash size')
    slots = device.slots
    if [p.subtype for p in slots] != ['ota_0', 'ota_1'] or slots[0].size != slots[1].size:
        raise ManifestError('there must be slots ota_0 then ota_1 of equal size, so either holds any image')
    if 'otadata' not in names or device.partition('otadata').end > slots[0].offset:
        raise ManifestError('an otadata partition must come before the application slots')
    if device.partition('otadata').size != 2 * ERASE_SECTOR:
        raise ManifestError('otadata must be two sectors, one per OTA selection record')
    if device.app_partition not in {p.name for p in slots}:
        raise ManifestError(f'[build].app_partition {device.app_partition!r} is not an application slot')


def check_owned(device):
    names = {p.name for p in device.partitions}
    for r in device.flash:
        if r.within not in names:
            raise ManifestError(f'owned range {r.region} is within unknown partition {r.within!r}')
        partition = device.partition(r.within)
        if not partition.offset <= r.offset < r.end <= partition.end or r.size == 0:
            raise ManifestError(f'owned range {r.region} must lie inside its partition {r.within}')
        if not aligned(r.offset, r.size):
            raise ManifestError(f'owned range {r.region} must be aligned to the {ERASE_SECTOR:#x}-byte erase sector')
    ranges = sorted(device.flash, key=lambda r: r.offset)
    for a, b in itertools.pairwise(ranges):
        if a.end > b.offset:
            raise ManifestError(f'owned ranges {a.region} and {b.region} overlap')
    holding = {r.within for r in ranges} & {slot.name for slot in device.slots}
    if ranges and len(holding) != 1:
        raise ManifestError('owned ranges must close exactly one application slot, so a flash preserves one span')
    for slot in device.slots:
        inside = [r for r in ranges if r.within == slot.name]
        if not inside:
            continue
        ends = [r.offset for r in inside[1:]] + [slot.end]
        if any(r.end != end for r, end in zip(inside, ends, strict=True)):
            raise ManifestError(f'owned ranges in {slot.name} must close the slot back to back, above the image')
    if device.image_cap == 0:
        raise ManifestError('owned ranges leave no room for the application image')


def check_panel(model):
    width, height = model.width, model.height
    if not (16 <= width <= MAX_PANEL[0] and 16 <= height <= MAX_PANEL[1]):
        raise ManifestError(f'panel {width}x{height} is outside 16x16 to {MAX_PANEL[0]}x{MAX_PANEL[1]}')
    if scale_quarters(width, height) < MIN_SCALE_QUARTERS:
        raise ManifestError(f'panel {width}x{height} would lay out below 3/4 scale')


def scale_quarters(width, height):
    return min(4 * width // BASE_PANEL[0], 4 * height // BASE_PANEL[1])


def check_buttons(model):
    ids = [b.id for b in model.buttons]
    if len(ids) < MIN_BUTTONS:
        raise ManifestError(f'a device needs at least {MIN_BUTTONS} buttons, one per role')
    if len(ids) > MAX_BUTTONS:
        raise ManifestError(f'a device has at most {MAX_BUTTONS} buttons, device_api::input::MAX_BUTTONS')
    if any(type(i) is not int or not 0 <= i <= MAX_BUTTON_ID for i in ids):
        raise ManifestError(f'button ids are integers from 0 to {MAX_BUTTON_ID}')
    if len(set(ids)) != len(ids):
        raise ManifestError('two buttons share an id')
    roles = [b.role for b in model.buttons if b.role is not None]
    if unknown := set(roles) - set(ROLES):
        raise ManifestError(f'unknown button roles: {", ".join(sorted(unknown))}')
    if sorted(roles) != sorted(ROLES):
        raise ManifestError(f'each of {", ".join(ROLES)} must be mapped to exactly one button')
    for b in model.buttons:
        if b.edge not in SIDES:
            raise ManifestError(f'button {b.id} edge {b.edge!r} must be one of {", ".join(SIDES)}')
        if type(b.at) is not int or not 0 <= b.at <= ALONG:
            raise ManifestError(
                f'button {b.id} at {b.at!r} must be an integer from 0 to {ALONG}, thousandths along its edge'
            )
        if b.role is not None and b.at in MIDDLE:
            raise ManifestError(
                f'button {b.id} ({b.role}) at {b.at} sits in the middle third of its edge '
                f'({MIDDLE.start}..{MIDDLE.stop - 1}): a mid-edge cue is not designed yet, so move it toward a corner'
            )
    beside = {}
    for b in model.buttons:
        if b.role is None:
            continue
        corner = nearest_corner(b.edge, b.at)
        if other := beside.get(corner):
            raise ManifestError(
                f'buttons {other.id} ({other.role}) and {b.id} ({b.role}) sit nearest the same corner, {corner}: '
                "each role's cue needs a corner of its own"
            )
        beside[corner] = b


def nearest_corner(edge, at):
    """The corner a cue takes beside a button, as `Corner::nearest` in vana's `screens::cue`."""
    far = at > ALONG // 2
    return {
        ('left', False): 'top-left',
        ('top', False): 'top-left',
        ('left', True): 'bottom-left',
        ('bottom', False): 'bottom-left',
        ('right', False): 'top-right',
        ('top', True): 'top-right',
        ('right', True): 'bottom-right',
        ('bottom', True): 'bottom-right',
    }[edge, far]


def manifests():
    return sorted(DEVICES.glob('*/device.toml'))


def models(found_devices):
    found = [device.model for device in found_devices]
    found += [parse_test_model(path) for path in sorted(TEST_MODELS.glob('*.toml'))]
    names = [model.name for model in found]
    if shared := sorted({name for name in names if names.count(name) > 1}):
        raise ManifestError(f'two models share a name: {", ".join(shared)}')
    return found


def selected(name=None):
    name = name or os.environ.get('CYCLING_DEVICE') or None
    if name is None:
        found = manifests()
        if len(found) != 1:
            names = ', '.join(p.parent.name for p in found)
            raise ManifestError(f'pass --device: there are {len(found)} device manifests ({names})')
        return parse(found[0])
    path = DEVICES / name / 'device.toml'
    if not path.is_file():
        raise ManifestError(f'no device manifest {path.relative_to(ROOT)}')
    return parse(path)


def rust_bool(value):
    return 'true' if value else 'false'


def rust_product(model):
    return f'Product {{ name: {json.dumps(model.product)}, fit_id: {model.fit_product} }}'


def rust_fixtures(model):
    def role(b):
        return 'None' if b.role is None else f'Some(Role::{b.role.capitalize()})'

    return (
        'Fixtures::new(&['
        + ''.join(
            f'Fixture {{ id: {b.id}, role: {role(b)}, edge: Side::{b.edge.capitalize()}, at: {b.at} }},'
            for b in model.buttons
        )
        + '])'
    )


def rust_sensors(model):
    return 'SensorSet { ' + ', '.join(f'{s}: {rust_bool(model.fitted[s])}' for s in SENSORS) + ' }'


def fitted_rs(device):
    model = device.model
    if model.memory:
        raise ManifestError(
            f'owned range {model.memory[0].region} is memory-backed, and {device.name} has no driver that backs it yet'
        )
    facts = ''.join(
        f'pub const {fact.upper()}: bool = {rust_bool(model.fitted[fact])};\n' for fact in FITTED if fact not in SENSORS
    )
    regions = ', '.join(f'("{r}", Region::{r.capitalize()})' for r in REGIONS)
    ranges = sorted(device.owned, key=lambda r: REGIONS.index(r.region))
    arms = '\n'.join(f'        Region::{r.region.capitalize()} => ({r.offset:#x}, {r.size:#x}),' for r in ranges)
    durable = '\n'.join(f'        Region::{r.region.capitalize()} => {rust_bool(r.durable)},' for r in ranges)
    text = f"""//! Generated by `cycling devices` from devices/{device.name}/device.toml; edit the manifest, then rerun it.

use device_api::input::{{Fixture, Fixtures, Role, Side}};
use device_api::sensors::SensorSet;
use device_api::storage::Region;

pub const NAME: &str = "{device.name}";
pub const PRODUCT_NAME: &str = {json.dumps(model.product)};
pub const FIT_PRODUCT: u16 = {model.fit_product};
pub const PANEL_WIDTH: usize = {model.width};
pub const PANEL_HEIGHT: usize = {model.height};
pub const BUTTONS: Fixtures = {rust_fixtures(model)};
{facts}pub const SENSORS: SensorSet = {rust_sensors(model)};
pub const REGIONS: &[(&str, Region)] = &[{regions}];

pub const fn owned(region: Region) -> (u32, usize) {{
    match region {{
{arms}
    }}
}}

pub const fn durable(region: Region) -> bool {{
    match region {{
{durable}
    }}
}}
"""
    return rustfmt(text)


def models_rs(found):
    def constant(model):
        name = const_name(model)
        facts = ', '.join(f'{fact}: {rust_bool(model.fitted[fact])}' for fact in FITTED if fact not in SENSORS)
        memory = ', '.join(f'(Region::{r.region.capitalize()}, {r.size:#x})' for r in model.memory)
        return f"""const {name}_BUTTONS: Fixtures = {rust_fixtures(model)};
static {name}_LAYOUT: Layout = Layout::of(Scaled::of({model.width}, {model.height}), &{name}_BUTTONS);
pub const {name}: Model = Model {{
    name: "{model.name}",
    product: {rust_product(model)},
    width: {model.width},
    height: {model.height},
    touch: {rust_bool(model.touch)},
    buttons: {name}_BUTTONS,
    layout: &{name}_LAYOUT,
    fitted: Fitted {{ {facts}, sensors: {rust_sensors(model)} }},
    memory: &[{memory}],
}};
"""

    every = ', '.join(f'&{const_name(model)}' for model in found)
    text = f"""//! Generated by `cycling devices` from devices/*/device.toml and tools/simulator/models/*.toml; edit those, then rerun it.

use crate::devices::{{Fitted, Model}};
use device_api::input::{{Fixture, Fixtures, Role, Side}};
use device_api::sensors::SensorSet;
use device_api::storage::Region;
use vana::Product;
use vana::screens::Layout;
use vana::screens::scaled::Scaled;

{''.join(constant(model) for model in found)}
pub fn all() -> &'static [&'static Model] {{
    &[{every}]
}}
"""
    return rustfmt(text)


def limits_rs(found_devices, found_models):
    def largest(models):
        return max(m.width for m in models), max(m.height for m in models)

    def panels(models):
        return ', '.join(f'({w}, {h})' for w, h in sorted({(m.width, m.height) for m in models}))

    real = largest([device.model for device in found_devices])
    tested = largest(found_models)
    text = f"""//! Generated by `cycling devices` from devices/*/device.toml and tools/simulator/models/*.toml; edit those, then rerun it.

/// The largest panel a device declares. Arrays sized by a panel use it, so
/// firmware grows only when a larger device arrives.
#[cfg(not(any(test, feature = "invariants")))]
pub const WIDTH: usize = {real[0]};
#[cfg(not(any(test, feature = "invariants")))]
pub const HEIGHT: usize = {real[1]};
/// The largest panel any model declares, the simulator's test models too.
#[cfg(any(test, feature = "invariants"))]
pub const WIDTH: usize = {tested[0]};
#[cfg(any(test, feature = "invariants"))]
pub const HEIGHT: usize = {tested[1]};

/// Every panel a device declares, as width and height.
#[cfg(not(any(test, feature = "invariants")))]
pub const PANELS: &[(usize, usize)] = &[{panels([device.model for device in found_devices])}];
/// Every panel any model declares, the simulator's test models too.
#[cfg(any(test, feature = "invariants"))]
pub const PANELS: &[(usize, usize)] = &[{panels(found_models)}];
"""
    return rustfmt(text)


def const_name(model):
    return model.name.upper().replace('-', '_')


def rustfmt(text):
    formatted = subprocess.run(
        ['rustfmt', '--edition', '2024', '--emit', 'stdout'], input=text, capture_output=True, text=True, check=True
    )
    return formatted.stdout


def generated(device):
    return {device.root / 'src' / 'fitted.rs': fitted_rs(device)}


def outputs():
    found_devices = [parse(path) for path in manifests()]
    every = {}
    for device in found_devices:
        every |= generated(device)
    found_models = models(found_devices)
    return every | {MODELS_RS: models_rs(found_models), LIMITS_RS: limits_rs(found_devices, found_models)}


def stale(device):
    return [path for path, text in generated(device).items() if not path.is_file() or path.read_text() != text]


def generate(locked):
    written = []
    for target, text in outputs().items():
        if target.is_file() and target.read_text() == text:
            continue
        written.append(target.relative_to(ROOT))
        if not locked:
            target.write_text(text)
    for path in written:
        print(f'{"stale" if locked else "wrote"} {path}', file=sys.stderr if locked else sys.stdout)
    if locked and written:
        print('Run cycling devices and commit the result.', file=sys.stderr)
        return 1
    return 0


def expected_fitted(device):
    fields = {
        'device': device.name,
        'width': device.model.width,
        'height': device.model.height,
        'buttons': ','.join(str(b.id) if b.role is None else f'{b.id}:{b.role}' for b in device.model.buttons),
    }
    for r in device.flash:
        fields |= {f'{r.region}_offset': r.offset, f'{r.region}_size': r.size, f'{r.region}_erase': ERASE_SECTOR}
    return fields


def compare_fitted(device, reply):
    if reply.get('device') != device.name:
        return [f'wrong device: the unit is {reply.get("device")!r}, the manifest is {device.name!r}']
    problems = []
    for key, want in expected_fitted(device).items():
        got = reply.get(key)
        if isinstance(want, int):
            try:
                got = int(got, 0)
            except TypeError, ValueError:
                pass
        if got != want:
            problems.append(f'{key}: unit {reply.get(key)!r}, manifest {want!r}')
    fitted = reply.get('touch') not in (None, 'unsupported')
    if fitted != device.model.touch:
        problems.append(f'touch: unit {reply.get("touch")!r}, manifest fitted={device.model.touch}')
    shutdown = reply.get('shutdown')
    if shutdown == 'unknown':
        print('note: shutdown=unknown is a runtime state, not a fitting; the unit cannot tell yet')
    elif shutdown != ('supported' if device.model.fitted['shutdown'] else 'unsupported'):
        problems.append(f'shutdown: unit {shutdown!r}, manifest fitted={device.model.fitted["shutdown"]}')
    for sensor in SENSORS:
        want = device.model.fitted[sensor]
        if reply.get(sensor) != ('fitted' if want else 'absent'):
            problems.append(f'{sensor}: unit {reply.get(sensor)!r}, manifest fitted={want}')
    return problems


def check_unit(device, port):
    log = ROOT / '.local' / 'devices' / f'check-{time.time_ns()}.log'
    log.parent.mkdir(parents=True, exist_ok=True)
    with UsbConnection(port, log) as connection:
        reply = connection.terminal_command('FITTED')
    if reply['status'] != 'OK':
        print(f'FITTED answered {reply["status"]}: {reply.get("data", "")}', file=sys.stderr)
        return 1
    print(f'FITTED {reply["data"]}')
    problems = compare_fitted(device, reply_fields(reply['data'], unique=True))
    for problem in problems:
        print(f'mismatch: {problem}', file=sys.stderr)
    if not problems:
        print(f'The unit matches devices/{device.name}/device.toml.')
    return 1 if problems else 0


def build_env(device, variant):
    architecture = ARCHITECTURES[device.target]
    values = {
        'device': device.name,
        'device_dir': device.root.relative_to(ROOT),
        'package': device.package,
        'bin': device.bin,
        'target': device.target,
        'chip': device.chip,
        'features': device.features(variant),
        'variant': variant or '',
        'flash_size': f'{device.flash_size >> 20}mb',
        'flash_mode': device.flash_mode,
        'flash_freq': device.flash_freq,
        'app_partition': device.app_partition,
        'stack_budget': device.stack_budget,
        'nm': architecture['nm'],
        'stack_start_symbol': architecture['stack_start'],
        'stack_end_symbol': architecture['stack_end'],
    }
    return ''.join(f'{key}={shlex.quote(str(value))}\n' for key, value in values.items())


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--locked', action='store_true', help='fail on stale generated files instead of writing')
    mode.add_argument(
        '--check',
        action='store_true',
        help="compare the attached unit's FITTED reply, and so the image it runs, with its manifest",
    )
    mode.add_argument('--build-env', metavar='PARTITIONS', type=Path, help=argparse.SUPPRESS)
    parser.add_argument('--device', help='the manifest for --check and --build-env')
    parser.add_argument('--port', help='--check: the tty; defaults to CYCLING_PORT, then the backed-up device')
    parser.add_argument('--variant', help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    try:
        if args.build_env:
            device = selected(args.device)
            if outdated := stale(device):
                names = ', '.join(str(path.relative_to(ROOT)) for path in outdated)
                raise ManifestError(f'{names} no longer match the manifest: run cycling devices before building')
            args.build_env.write_text(device.partition_csv())
            print(build_env(device, args.variant or None), end='')
            return 0
        if args.check:
            return check_unit(selected(args.device), select_port(args.port))
        return generate(args.locked)
    except ManifestError as error:
        print(f'cycling devices: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
