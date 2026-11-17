"""Device manifests: devices/<name>/device.toml declares a model once, and everything else derives from it.

    cycling devices                 # check every manifest; write each devices/<name>/src/fitted.rs and the simulator's models.rs
    cycling devices --locked        # fail, naming each stale file, instead of writing
    cycling devices --check         # compare the attached unit's FITTED reply with its manifest

build.sh, flash, backup and the other device tasks read the manifest that
`--device <name>` (CYCLING_DEVICE) selects; it may be left out while exactly one exists.

Test-only models in tools/simulator/models/<name>.toml declare only [device], [panel] and
[controls]. They feed tools/simulator/src/models.rs and nothing that builds firmware.
"""

import argparse
import itertools
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
ERASE_SECTOR = 0x1000
HARDWARE_FEATURE = 'hardware'
REGIONS = ('configuration', 'data', 'crash', 'intent')
BUTTONS_BY_ID = ('TopLeft', 'BottomLeft', 'BottomRight', 'Center')
ROLES = ('act', 'back', 'move')
MIN_BUTTONS = 3
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

    @property
    def end(self):
        return self.offset + self.size


@dataclass(frozen=True)
class Fixture:
    id: int
    role: str | None


@dataclass(frozen=True)
class Model:
    name: str
    width: int
    height: int
    touch: bool
    buttons: tuple[Fixture, ...]


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
    owned: tuple[Owned, ...]
    model: Model

    @property
    def name(self):
        return self.model.name

    def partition(self, name):
        return next(p for p in self.partitions if p.name == name)

    def region(self, name):
        return next(r for r in self.owned if r.region == name)

    @property
    def slots(self):
        return tuple(p for p in self.partitions if p.type == 'app')

    @property
    def image_cap(self):
        room = [p.size for p in self.slots]
        for slot in self.slots:
            room += [r.offset - slot.offset for r in self.owned if r.within == slot.name]
        return min(room)

    @property
    def owned_tail(self):
        inside = [r for r in self.owned if r.within in {p.name for p in self.slots}]
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
    table(raw, 'the manifest', ('device', 'build', 'partitions', 'owned', 'panel', 'controls'))
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
    owned = tuple(
        Owned(region, **table(value, f'[owned].{region}', ('offset', 'size', 'within')))
        for region, value in table(raw['owned'], '[owned]', REGIONS).items()
    )
    model = build_model(raw)
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


def build_model(raw):
    identity = table(raw['device'], '[device]', ('name',))
    panel = table(raw['panel'], '[panel]', ('width', 'height'))
    controls = table(raw['controls'], '[controls]', ('touch', 'buttons'))
    buttons = tuple(
        Fixture(**{'role': None} | table(button, f'[controls] button {index}', ('id',), optional=('role',)))
        for index, button in enumerate(controls['buttons'])
    )
    return Model(identity['name'], panel['width'], panel['height'], controls['touch'], buttons)


def parse_test_model(path):
    try:
        raw = tomllib.loads(path.read_text())
        table(raw, 'the model', ('device', 'panel', 'controls'))
        model = build_model(raw)
        if model.name != path.stem:
            raise ManifestError(f'[device].name {model.name!r} must match its file name {path.stem!r}')
        check_name(model)
        check_panel(model)
        check_buttons(model)
    except (tomllib.TOMLDecodeError, ManifestError) as error:
        raise ManifestError(f'{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {error}') from None
    return model


def check(device, root):
    if device.name != root.name:
        raise ManifestError(f'[device].name {device.name!r} must match its directory {root.name!r}')
    check_name(device.model)
    check_cargo(device, root / 'Cargo.toml')
    check_partitions(device)
    check_owned(device)
    check_panel(device.model)
    check_buttons(device.model)


def check_name(model):
    if not re.fullmatch(NAME, model.name):
        raise ManifestError(f'[device].name {model.name!r} must match {NAME}')
    if model.name in RESERVED_NAMES:
        raise ManifestError(f"[device].name {model.name!r} is reserved for the simulator's sized shell sessions")


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
    for r in device.owned:
        if r.within not in names:
            raise ManifestError(f'owned range {r.region} is within unknown partition {r.within!r}')
        partition = device.partition(r.within)
        if not partition.offset <= r.offset < r.end <= partition.end or r.size == 0:
            raise ManifestError(f'owned range {r.region} must lie inside its partition {r.within}')
        if not aligned(r.offset, r.size):
            raise ManifestError(f'owned range {r.region} must be aligned to the {ERASE_SECTOR:#x}-byte erase sector')
    ranges = sorted(device.owned, key=lambda r: r.offset)
    for a, b in itertools.pairwise(ranges):
        if a.end > b.offset:
            raise ManifestError(f'owned ranges {a.region} and {b.region} overlap')
    holding = {r.within for r in ranges} & {slot.name for slot in device.slots}
    if len(holding) != 1:
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
    if len(set(ids)) != len(ids):
        raise ManifestError('two buttons share an id')
    if any(not 0 <= i < len(BUTTONS_BY_ID) for i in ids):
        raise ManifestError(f'button ids run from 0 to {len(BUTTONS_BY_ID) - 1}')
    if ids != sorted(ids):
        raise ManifestError('list buttons in ascending id order: a trace records them as a set')
    roles = [b.role for b in model.buttons if b.role is not None]
    if unknown := set(roles) - set(ROLES):
        raise ManifestError(f'unknown button roles: {", ".join(sorted(unknown))}')
    if sorted(roles) != sorted(ROLES):
        raise ManifestError(f'each of {", ".join(ROLES)} must be mapped to exactly one button')


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


def fitted_rs(device):
    buttons = ', '.join(f'Button::{BUTTONS_BY_ID[b.id]}' for b in device.model.buttons)
    regions = ', '.join(f'("{r}", Region::{r.capitalize()})' for r in REGIONS)
    arms = '\n'.join(
        f'        Region::{r.region.capitalize()} => ({r.offset:#x}, {r.size:#x}),'
        for r in sorted(device.owned, key=lambda r: REGIONS.index(r.region))
    )
    text = f"""//! Generated by `cycling devices` from devices/{device.name}/device.toml; edit the manifest, then rerun it.

use device_api::input::Button;
use device_api::storage::Region;

pub const NAME: &str = "{device.name}";
pub const PANEL_WIDTH: usize = {device.model.width};
pub const PANEL_HEIGHT: usize = {device.model.height};
pub const BUTTONS: &[Button] = &[{buttons}];
pub const REGIONS: &[(&str, Region)] = &[{regions}];

pub const fn owned(region: Region) -> (u32, usize) {{
    match region {{
{arms}
    }}
}}
"""
    return rustfmt(text)


def models_rs(found):
    def constant(model):
        buttons = ', '.join(f'Button::{BUTTONS_BY_ID[b.id]}' for b in model.buttons)
        return f"""pub const {const_name(model)}: Model = Model {{
    name: "{model.name}",
    width: {model.width},
    height: {model.height},
    touch: {'true' if model.touch else 'false'},
    buttons: &[{buttons}],
}};
"""

    every = ', '.join(f'&{const_name(model)}' for model in found)
    text = f"""//! Generated by `cycling devices` from devices/*/device.toml and tools/simulator/models/*.toml; edit those, then rerun it.

use crate::devices::Model;
use device_api::input::Button;

{''.join(constant(model) for model in found)}
pub fn all() -> &'static [&'static Model] {{
    &[{every}]
}}
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
    return every | {MODELS_RS: models_rs(models(found_devices))}


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
        'buttons': ','.join(str(b.id) for b in device.model.buttons),
    }
    for r in device.owned:
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
