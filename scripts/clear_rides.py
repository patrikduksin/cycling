"""Clear the owned ride journal only after verifying a complete local export."""
import argparse
import hashlib
import json
import time
from pathlib import Path

from export_rides import info, read_slot
from usb import ROOT, UsbConnection


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


def status_fields(reply):
    if reply['status'] != 'OK':
        raise RuntimeError(f'ride status rejected: {reply["status"]}')
    try:
        return dict(field.split('=', 1) for field in reply['data'].split())
    except ValueError:
        raise ValueError('malformed ride status') from None


def mutate_once(connection, command, timeout=90):
    """Accept once, then poll only status until that token completes."""
    deadline = time.monotonic() + timeout
    reply = connection.terminal_command(command, timeout=min(8, timeout))
    if reply['status'] != 'ACCEPTED':
        raise RuntimeError(f'ride mutation rejected: {reply["status"]}')
    try:
        accepted = dict(field.split('=', 1) for field in reply['data'].split())
        token = int(accepted['token'])
        if not 1 <= token <= 0xffffffff:
            raise ValueError()
    except (KeyError, ValueError):
        raise RuntimeError('mutation accepted with an invalid token; outcome is uncertain') from None
    while time.monotonic() < deadline:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        result = status_fields(connection.terminal_command('RIDE STATUS', timeout=min(8, remaining)))
        if 'completed' in result:
            if result['completed'] != str(token):
                raise RuntimeError('different mutation completed; outcome is uncertain')
            if result.get('result') != 'OK':
                raise RuntimeError('ride mutation failed; inspect status before any retry')
            return result
        if result.get('pending') != str(token):
            raise RuntimeError('mutation completion is missing; outcome is uncertain')
        time.sleep(min(.1, max(0, deadline - time.monotonic())))
    raise TimeoutError('ride mutation completion deadline expired')


def clear(port, export_directory, output):
    manifest = verified_manifest(export_directory)
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    # Keep the same advisory lock and no-reset descriptor through the complete
    # verification, accepted mutation and completion query. No lease handoff.
    with UsbConnection(port, output / 'usb.log') as connection:
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
            result = mutate_once(connection, f'RIDE CLEAR CONFIRM {manifest["upper_bound"]}')
        except TimeoutError:
            raise RuntimeError(
                'clear outcome is uncertain; inspect RIDE STATUS after reboot and do not retry '
                'until scanning completes') from None
        if result.get('state') != 'ready' or result.get('slot') != '0':
            raise RuntimeError('clear completed without an empty ready journal')
        after = info(connection)
        if after['upper_bound'] != 0 or after['status'] != 'ready':
            raise RuntimeError('clear readback did not report an empty ready journal')
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
