"""Resolve the backed-up C606's current Linux tty through read-only sysfs metadata."""
import json
from pathlib import Path
import re
import sys

from cycling_devtools.workspace import ROOT


class DiscoveryError(ValueError):
    """Messages contain no device identities or private manifest contents."""


def normalize_identity(value):
    if not isinstance(value, str):
        raise DiscoveryError('device identity is invalid')
    normalized = value.strip().lower().replace(':', '').replace('-', '')
    if not re.fullmatch(r'[0-9a-f]{12}', normalized):
        raise DiscoveryError('device identity is invalid')
    return normalized


def resolve_port(sysfs=Path('/sys/class/tty'), manifest=ROOT / '.local/device/manifest.json'):
    try:
        original = normalize_identity(json.loads(Path(manifest).read_text())['identity'])
    except (OSError, ValueError, KeyError, TypeError):
        raise DiscoveryError('private backup manifest is unavailable or invalid') from None
    matches = []
    try:
        candidates = list(Path(sysfs).glob('ttyACM*'))
    except OSError:
        raise DiscoveryError('tty sysfs metadata is unavailable') from None
    for tty in candidates:
        if not re.fullmatch(r'ttyACM[0-9]+', tty.name):
            continue
        try:
            device = tty.resolve(strict=True)
            for ancestor in (device, *device.parents):
                vendor, product = ancestor / 'idVendor', ancestor / 'idProduct'
                if not vendor.exists() or not product.exists():
                    continue
                # Stop at the first USB device, never attribute a child to its hub.
                if vendor.read_text().strip().lower() == '303a' and product.read_text().strip().lower() == '1001':
                    serial = normalize_identity((ancestor / 'serial').read_text())
                    if serial == original:
                        matches.append(Path('/dev') / tty.name)
                break
        except (OSError, ValueError, RuntimeError):
            # A disappeared or incomplete device cannot count as a verified match.
            continue
    if len(matches) != 1:
        raise DiscoveryError('expected exactly one C606 matching the private backup manifest')
    return matches[0]


def main():
    try:
        print(resolve_port())
    except DiscoveryError as error:
        print(f'Device port discovery failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
