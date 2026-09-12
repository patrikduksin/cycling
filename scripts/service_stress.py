"""Bounded ordinary-terminal progress checks, valid with either harness mode."""
import argparse
import os
import json
import re
import time
from pathlib import Path

from usb import ROOT, UsbConnection

FAULTS = {
    'POSITION': ('checksum_errors', 'parse_errors', 'dma_losses', 'line_overflows', 'uart_errors'),
    'INPUT': ('bad_crc', 'uart_errors', 'touch_errors', 'input_lost'),
}
PROGRESS = {'POSITION': ('bytes', 'valid'), 'INPUT': ('companion_valid',)}
PREFERENCES = ('brightness', 'dim_timeout', 'dim_brightness', 'timezone')


def fields(reply):
    if reply.get('status') != 'OK':
        raise AssertionError(f"terminal rejected request: {reply.get('status')}")
    result = dict(re.findall(r'(\w+)=([^\s]+)', reply['data']))
    result['ms'] = reply['ms']
    return result


def read(connection, command):
    return fields(connection.terminal_command(command))


def validate(command, before, after, allow_loss=False):
    keys = PROGRESS[command] + FAULTS[command]
    delta = {key: int(after[key]) - int(before[key]) for key in keys}
    if any(value < 0 for value in delta.values()) or int(after['ms']) <= int(before['ms']):
        raise AssertionError('device counters/time reset during measurement')
    if any(delta[key] <= 0 for key in PROGRESS[command]):
        raise AssertionError(f'{command} acquisition did not advance')
    if not allow_loss and any(delta[key] for key in FAULTS[command]):
        raise AssertionError(f'{command} faults advanced: {delta}')
    return delta


def private_directory(directory):
    directory = Path(directory).resolve()
    if not directory.is_relative_to((ROOT / '.local').resolve()):
        raise ValueError('evidence must stay in ignored .local/')
    directory.mkdir(parents=True, exist_ok=False, mode=0o700)
    return directory


def run(port, directory, seconds=20, command='POSITION', interval=.05, allow_loss=False):
    if command not in PROGRESS or not 2 <= seconds <= 600 or not .01 <= interval <= 5:
        raise ValueError('command must be POSITION/INPUT, seconds 2..600, interval .01..5')
    directory = private_directory(directory)
    with UsbConnection(port, directory / 'raw.bin') as connection:
        build = read(connection, 'INFO')
        settings = read(connection, 'SETTINGS')
        system = read(connection, 'STATUS')
        inventory = read(connection, 'RIDE STATUS') if build.get('cycling') == 'true' else None
        before = read(connection, command)
        deadline = time.monotonic() + seconds
        samples = 0
        sampled_minimum = int(system['heap_free'])
        after = before
        while time.monotonic() < deadline:
            time.sleep(min(interval, max(0, deadline - time.monotonic())))
            after = read(connection, command)
            samples += 1
            if samples % 20 == 0:
                sampled_minimum = min(sampled_minimum, int(read(connection, 'STATUS')['heap_free']))
        final_settings = read(connection, 'SETTINGS')
        final_system = read(connection, 'STATUS')
        sampled_minimum = min(sampled_minimum, int(final_system['heap_free']))
        result = dict(command=command, build=build, recording=False,
                      requested_seconds=seconds, device_elapsed_ms=None,
                      samples=samples, before=before, after=after,
                      settings_before=settings, settings_after=final_settings,
                      status_before=system, status_after=final_system,
                      inventory_before=inventory, inventory_after=None,
                      sampled_heap_min=sampled_minimum, final_heap_free=int(final_system['heap_free']),
                      delta=None, ok=False, error=None)
        try:
            # Keep computable deltas even when a progress/loss assertion fails.
            result['delta'] = {key: int(after[key]) - int(before[key])
                               for key in PROGRESS[command] + FAULTS[command]}
            result['device_elapsed_ms'] = after['ms'] - before['ms']
            for key in PREFERENCES:
                if final_settings[key] != settings[key]:
                    raise AssertionError(f'preference changed: {key}')
            if final_system['storage_ops'] != system['storage_ops']:
                raise AssertionError('storage operations occurred during read-only stress')
            if inventory is not None:
                final_inventory = read(connection, 'RIDE STATUS')
                result['inventory_after'] = final_inventory
                for key in ('slot', 'rides', 'samples', 'pending'):
                    if inventory[key] != final_inventory[key]:
                        raise AssertionError(f'ride inventory changed: {key}')
            validate(command, before, after, allow_loss)
            result['ok'] = True
        except Exception as error:
            result['error'] = dict(type=type(error).__name__, message=str(error))
            raise
        finally:
            (directory / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('service', choices=('gps', 'companion'))
    parser.add_argument('directory')
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--seconds', type=float, default=20)
    parser.add_argument('--interval', type=float, default=.05)
    parser.add_argument('--allow-loss', action='store_true')
    args = parser.parse_args()
    command = {'gps': 'POSITION', 'companion': 'INPUT'}[args.service]
    print(json.dumps(run(args.port, args.directory, args.seconds, command,
                         args.interval, args.allow_loss), indent=2))


if __name__ == '__main__':
    main()
