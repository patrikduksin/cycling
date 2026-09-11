"""Measure companion UART progress and faults in a bounded device window."""
import argparse
import json
import time
from pathlib import Path

from debug import Device, wake_if_dimmed


PRESERVED = ('brightness', 'dim_timeout', 'dim_brightness', 'timezone',
             'recorded_rides', 'recording_slot')
COUNTERS = ('valid', 'bad_crc', 'uart_errors')


def go_home(device):
    wake_if_dimmed(device)
    state = device.command('STATE')
    for _ in range(3):
        if state['screen'] == 'home':
            return state
        state = device.command('BUTTON 0 1')
    if state['screen'] == 'home':
        return state
    raise AssertionError(f'cannot return home from {state["screen"]}')


def set_temporary_brightness(device, brightness):
    go_home(device)
    device.tap(80, 210)
    device.expect({'screen': 'controls'})
    x = 24 + ((brightness - 5) * 192 + 94) // 95
    device.drag([216, 260], [x, 260], seconds=.25, steps=4)
    return device.expect({'brightness': brightness})


def snapshot(state):
    keys = PRESERVED + COUNTERS + ('ms', 'frame', 'wifi', 'ble_state',
                                   'gps_valid', 'gps_uart_errors', 'heap_free')
    return {key: state[key] for key in keys}


def measure(port, output, seconds, interval, brightness, wifi_reconnect=False):
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    with Device(port, output / 'session') as device:
        original = device.command('STATE')
        if brightness is not None:
            set_temporary_brightness(device, brightness)
        baseline = device.command('STATE')
        if wifi_reconnect:
            device.command('WIFI')
        deadline = time.monotonic() + seconds
        samples = []
        while time.monotonic() < deadline:
            device.wait(min(interval, deadline - time.monotonic()))
            samples.append(device.command('STATE'))
        final = device.command('STATE')
    restored = device.final
    for key in PRESERVED:
        if restored[key] != original[key]:
            raise AssertionError(f'test changed preserved field {key}')
    result = {
        'seconds': (final['ms'] - baseline['ms']) / 1000,
        'brightness': baseline['brightness'],
        'baseline': snapshot(baseline),
        'final': snapshot(final),
        'delta': {key: final[key] - baseline[key] for key in COUNTERS},
        'samples': len(samples),
    }
    (output / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--port', default='/dev/ttyACM0')
    parser.add_argument('--seconds', type=float, default=12)
    parser.add_argument('--interval', type=float, default=.1)
    parser.add_argument('--brightness', type=int, choices=range(5, 101))
    parser.add_argument('--allow-loss', action='store_true')
    parser.add_argument('--wifi-reconnect', action='store_true')
    args = parser.parse_args()
    if not 2 <= args.seconds <= 120 or not .05 <= args.interval <= 5:
        raise SystemExit('seconds must be 2..120 and interval .05..5')
    result = measure(args.port, args.output, args.seconds, args.interval, args.brightness,
                     args.wifi_reconnect)
    print(json.dumps(result, indent=2))
    if result['delta']['valid'] <= 0:
        raise SystemExit('companion packets did not advance')
    if not args.allow_loss and (result['delta']['uart_errors'] or result['delta']['bad_crc']):
        raise SystemExit('companion transport faults increased')


if __name__ == '__main__':
    main()
