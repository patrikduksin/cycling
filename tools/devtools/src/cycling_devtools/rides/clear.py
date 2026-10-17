"""Clear the owned ride journal only after verifying a complete local export.

`--medium bulk` clears the MMC ride log instead; the export must be a bulk
export of the same store epoch.
"""

import argparse
import hashlib
import json
import time
from pathlib import Path

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.rides.export import (
    BULK_DATA_START,
    MEDIA,
    bulk_data_start,
    info,
    read_owned_sector,
    read_slot,
)
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT


def file_sha256(path, size):
    """SHA-256 of a file that must be exactly `size` bytes, read in bounded chunks."""
    path = Path(path)
    if path.stat().st_size != size:
        return None
    digest = hashlib.sha256()
    with path.open('rb') as source:
        while chunk := source.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def verified_manifest(directory):
    directory = Path(directory)
    manifest = json.loads((directory / 'manifest.json').read_text())
    limit = manifest['capacity'] if manifest.get('medium') == 'bulk' else 4096
    if manifest['slot_size'] != 256 or not 0 <= manifest['upper_bound'] <= limit:
        raise ValueError('unsupported export bounds')
    # A bulk prefix can be gigabytes: check its length first, then hash it in chunks.
    digest = file_sha256(directory / Path(manifest['raw_file']).name, manifest['upper_bound'] * manifest['slot_size'])
    if digest is None:
        raise ValueError('export length does not match its manifest')
    if digest != manifest['raw_sha256']:
        raise ValueError('export digest does not match its manifest')
    if manifest.get('medium') == 'bulk':
        verify_invalid_sectors(directory, manifest)
    return manifest


def verify_invalid_sectors(directory, manifest):
    """Each invalid bulk container's raw sector is kept, intact, in the export."""
    containers = manifest.get('invalid_containers')
    sectors = manifest.get('invalid_sectors')
    if not isinstance(containers, list) or not isinstance(sectors, list):
        raise ValueError('bulk export lacks its invalid container list')
    if [entry.get('index') for entry in sectors] != containers:
        raise ValueError('bulk export invalid sectors do not match its invalid containers')
    for entry in sectors:
        index = entry['index']
        start = manifest.get('data_start', BULK_DATA_START)
        if not 0 <= index < manifest['upper_bound'] or entry.get('sector') != start + index:
            raise ValueError('bulk export invalid sector is out of bounds')
        path = directory / Path(entry['file']).name
        if not path.is_file():
            raise ValueError(f'bulk export lacks raw sector {entry["sector"]}')
        if file_sha256(path, 512) != entry['sha256']:
            raise ValueError(f'raw sector {entry["sector"]} does not match its manifest')


def status_command(medium):
    return 'RIDE BULK STATUS' if medium == 'bulk' else 'RIDE INTERNAL STATUS'


def medium_word(medium):
    if medium not in MEDIA:
        raise ValueError(f'unknown medium {medium}')
    return 'BULK ' if medium == 'bulk' else ''


def slot_bytes(slot, medium):
    return slot[0] if medium == 'bulk' else slot


def status_fields(reply):
    if reply['status'] != 'OK':
        raise RuntimeError(f'ride status rejected: {reply["status"]}')
    try:
        return dict(field.split('=', 1) for field in reply['data'].split())
    except ValueError:
        raise ValueError('malformed ride status') from None


def mutate_once(connection, command, timeout=90, status='RIDE STATUS'):
    """Accept once, then poll only status until that token completes."""
    deadline = time.monotonic() + timeout
    reply = connection.terminal_command(command, timeout=min(8, timeout))
    if reply['status'] != 'ACCEPTED':
        raise RuntimeError(f'ride mutation rejected: {reply["status"]}')
    try:
        accepted = dict(field.split('=', 1) for field in reply['data'].split())
        token = int(accepted['token'])
        if not 1 <= token <= 0xFFFFFFFF:
            raise ValueError()
    except KeyError, ValueError:
        raise RuntimeError('mutation accepted with an invalid token; outcome is uncertain') from None
    while time.monotonic() < deadline:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        result = status_fields(connection.terminal_command(status, timeout=min(8, remaining)))
        if 'completed' in result:
            if result['completed'] != str(token):
                raise RuntimeError('different mutation completed; outcome is uncertain')
            if result.get('result') != 'OK':
                raise RuntimeError('ride mutation failed; inspect status before any retry')
            return result
        if result.get('pending') != str(token):
            raise RuntimeError('mutation completion is missing; outcome is uncertain')
        time.sleep(min(0.1, max(0, deadline - time.monotonic())))
    raise TimeoutError('ride mutation completion deadline expired')


def clear(port, export_directory, output, medium='internal'):
    manifest = verified_manifest(export_directory)
    if manifest.get('medium', 'internal') != medium:
        raise ValueError(f'export is not a {medium} export')
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    # Keep the same advisory lock and no-reset descriptor through the complete
    # verification, accepted mutation and completion query. No lease handoff.
    with UsbConnection(port, output / 'usb.log') as connection:
        current = info(connection, medium)
        digest = hashlib.sha256()
        invalid = []
        for index in range(current['upper_bound']):
            slot = read_slot(connection, index, medium)
            if medium == 'bulk' and not slot[1]:
                invalid.append(index)
            digest.update(slot_bytes(slot, medium))
        # Zero placeholders stand in for invalid containers: the raw sectors
        # behind them must match what the export kept.
        start = bulk_data_start(connection) if invalid else None
        if invalid and start != manifest.get('data_start', BULK_DATA_START):
            raise RuntimeError('bulk records moved since the export; create and verify a new export')
        raw_sectors = [hashlib.sha256(read_owned_sector(connection, start + index)).hexdigest() for index in invalid]
        unchanged = info(connection, medium) == current
        if medium == 'bulk' and (
            invalid != manifest['invalid_containers']
            or raw_sectors != [entry['sha256'] for entry in manifest['invalid_sectors']]
        ):
            raise RuntimeError('device invalid sectors do not match the supplied export')
        if medium == 'bulk' and current.get('store_id') != manifest.get('store_id'):
            raise RuntimeError('bulk store changed after export; create and verify a new export')
        if not unchanged or current['upper_bound'] != manifest['upper_bound']:
            raise RuntimeError('journal advanced after export; create and verify a new export')
        if digest.hexdigest() != manifest['raw_sha256']:
            raise RuntimeError('device journal does not match the supplied export')
        try:
            result = mutate_once(
                connection,
                f'RIDE {medium_word(medium)}CLEAR CONFIRM {manifest["upper_bound"]}',
                status=status_command(medium),
            )
        except TimeoutError:
            raise RuntimeError(
                'clear outcome is uncertain; inspect RIDE STATUS after reboot and do not retry until scanning completes'
            ) from None
        if result.get('state') != 'ready' or result.get('slot') != '0':
            raise RuntimeError('clear completed without an empty ready journal')
        after = info(connection, medium)
        if after['upper_bound'] != 0 or after['status'] != 'ready':
            raise RuntimeError('clear readback did not report an empty ready journal')
    summary = {
        'export': str(export_directory),
        'medium': medium,
        'raw_sha256': manifest['raw_sha256'],
        'cleared_upper_bound': manifest['upper_bound'],
        'result': result,
    }
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('export_directory', type=Path)
    add_port_argument(parser)
    parser.add_argument('--output', type=Path, default=ROOT / '.local/tests' / f'ride-clear-{time.time_ns()}')
    parser.add_argument('--medium', choices=MEDIA, default='internal', help='ride journal to clear (default internal)')
    args = parser.parse_args()
    port = selected_port(parser, args.port)
    print(json.dumps(clear(port, args.export_directory, args.output, args.medium), indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        raise SystemExit(str(error)) from None
