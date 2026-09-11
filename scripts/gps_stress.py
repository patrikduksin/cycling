"""Measure GNSS transport progress during repeated complete USB state replies."""

import argparse
import json
import time
from pathlib import Path

from debug import Device


GPS_COUNTERS = (
    'gps_bytes', 'gps_valid', 'gps_checksum_errors', 'gps_parse_errors',
    'gps_overflows', 'gps_line_overflows', 'gps_uart_errors',
)


def run(port, directory, seconds, require_no_loss=False):
    directory = Path(directory)
    started = time.monotonic()
    replies = 0
    with Device(port, directory / 'session') as device:
        before = device.command('STATE')
        deadline = time.monotonic() + seconds
        after = before
        while time.monotonic() < deadline:
            after = device.command('STATE')
            replies += 1

        for key in ('recorded_rides', 'recording_slot', 'brightness', 'dim_timeout',
                    'dim_brightness', 'timezone'):
            if after[key] != before[key]:
                raise AssertionError(f'{key} changed: {before[key]} -> {after[key]}')
        if after['gps_bytes'] <= before['gps_bytes'] or after['gps_valid'] <= before['gps_valid']:
            raise AssertionError('GNSS parser did not advance')

        delta = {key: after[key] - before[key] for key in GPS_COUNTERS}
        faults = sum(delta[key] for key in GPS_COUNTERS[2:])
        result = {
            'ok': not require_no_loss or faults == 0,
            'build_harness': True,
            'recording': after['recording'],
            'requested_seconds': seconds,
            'elapsed_seconds': round(time.monotonic() - started, 3),
            'state_replies': replies,
            'before': before,
            'after': after,
            'delta': delta,
        }
        directory.mkdir(parents=True, exist_ok=True)
        (directory / 'gps-stress-summary.json').write_text(
            json.dumps(result, indent=2) + '\n')
        if not result['ok']:
            raise AssertionError(f'GNSS transport/parser faults advanced: {delta}')
        return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('directory')
    parser.add_argument('--port', default='/dev/ttyACM0')
    parser.add_argument('--seconds', type=float, default=20)
    parser.add_argument('--require-no-loss', action='store_true')
    args = parser.parse_args()
    print(json.dumps(run(args.port, args.directory, args.seconds, args.require_no_loss), indent=2))
