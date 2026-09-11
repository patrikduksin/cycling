"""Clear the owned ride journal only after verifying a complete local export."""
import argparse
import hashlib
import json
import time
from pathlib import Path

from debug import Device
from export_rides import ExportConnection, info, read_slot
from screenshot import ROOT


def verified_manifest(directory):
    directory = Path(directory)
    manifest = json.loads((directory / 'manifest.json').read_text())
    raw = (directory / manifest['raw_file']).read_bytes()
    if len(raw) != manifest['upper_bound'] * manifest['slot_size']:
        raise ValueError('export length does not match its manifest')
    if hashlib.sha256(raw).hexdigest() != manifest['raw_sha256']:
        raise ValueError('export digest does not match its manifest')
    if manifest['slot_size'] != 256 or not 0 <= manifest['upper_bound'] <= 4096:
        raise ValueError('unsupported export bounds')
    return manifest


def clear(port, export_directory, output):
    manifest = verified_manifest(export_directory)
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    with ExportConnection(port, output / 'before-usb.log') as connection:
        current = info(connection)
        digest = hashlib.sha256()
        for index in range(current['upper_bound']):
            digest.update(read_slot(connection, index))
        unchanged = info(connection) == current
    if not unchanged or current['upper_bound'] != manifest['upper_bound']:
        raise RuntimeError('journal advanced after export; create and verify a new export')
    if digest.hexdigest() != manifest['raw_sha256']:
        raise RuntimeError('device journal does not match the supplied export')
    try:
        with Device(port, output / 'clear') as device:
            # This durable command ends the temporary injection session before
            # erasing; suppress lease heartbeats and END while waiting.
            device.active = False
            result = device.command(
                f'RIDE CLEAR CONFIRM {manifest["upper_bound"]}', timeout=90)
            if result['ride_recording'] != 'ready' or result['recording_slot'] != 0:
                raise RuntimeError('clear acknowledged without an empty ready journal')
    except TimeoutError:
        raise RuntimeError(
            'clear outcome is uncertain; inspect STATE after reboot and do not retry '
            'until scanning completes') from None
    summary = {'export': str(export_directory), 'raw_sha256': manifest['raw_sha256'],
               'cleared_upper_bound': manifest['upper_bound'], 'result': result}
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('export_directory', type=Path)
    parser.add_argument('--port', default='/dev/ttyACM0')
    parser.add_argument('--output', type=Path,
                        default=ROOT / '.local/tests' / f'ride-clear-{time.time_ns()}')
    args = parser.parse_args()
    print(json.dumps(clear(args.port, args.export_directory, args.output), indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        raise SystemExit(str(error)) from None
