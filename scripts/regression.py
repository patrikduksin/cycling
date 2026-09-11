#!/usr/bin/env python3
"""Run the bounded C606 regression suite and retain all evidence under .local/."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import time

from debug import Device, ROOT, export, wake_if_dimmed
from visual import (
    DIAGNOSTICS_MASKS,
    DIAGNOSTICS_STATIC_REGIONS,
    compare,
    compare_regions,
    read_rgb565,
)


def run(command, log, *, expect_success=True):
    print('+', ' '.join(map(str, command)), flush=True)
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    log.write_text(result.stdout + result.stderr)
    if expect_success != (result.returncode == 0):
        raise RuntimeError(f'Unexpected exit {result.returncode}; inspect {log}')
    return result


def debug(port, output, name, *arguments, expect_success=True):
    return run(
        [sys.executable, 'scripts/debug.py', '--port', port, '--output', output / name, *arguments],
        output / f'{name}.log',
        expect_success=expect_success,
    )


def expected_canvas(output, scene):
    path = output / f'expected-{scene}.rgb565'
    run(
        ['cargo', 'run', '--locked', '--example', 'preview', '--', path, f'{scene}-raw'],
        output / f'expected-{scene}.log',
    )
    return read_rgb565(path)


def captured_canvas(output, name):
    report = json.loads((output / name / 'report.json').read_text())
    frame = report['frames'][-1]
    return read_rgb565(output / name / frame['raw_file'])


def assertion_cleanup(port, output):
    scenario = output / 'intentional-failure.json'
    scenario.write_text(json.dumps([
        {'action': 'command', 'value': 'BATTERY 8 3300 1'},
        {'action': 'tap', 'point': [80, 260]},
        {'action': 'expect', 'state': {'screen': 'intentionally-wrong'}},
    ]) + '\n')
    debug(port, output, 'assertion-failure', 'run', scenario, expect_success=False)
    report = json.loads((output / 'assertion-failure' / 'report.json').read_text())
    if report['ok'] or report['cleanup_error'] is not None:
        raise AssertionError('Intentional assertion did not perform clean session cleanup')
    if not report['error'].startswith("Expected {'screen': 'intentionally-wrong'}"):
        raise AssertionError(f"Scenario failed before deliberate assertion: {report['error']}")
    mutation = next(
        (event for event in report['events']
         if event['command'] == 'BATTERY 8 3300 1' and event['result'] == 'OK'),
        None,
    )
    if mutation is None or not mutation['state']['fake_battery']:
        raise AssertionError('Temporary mutation was not applied before deliberate assertion')
    baseline, final = report['baseline'], report['final']
    for key in ['screen', 'focus', 'brightness', 'dim_timeout', 'dim_brightness',
                'timezone', 'ride_phase', 'ride_page', 'ride_layout']:
        if final[key] != baseline[key]:
            raise AssertionError(f'Assertion cleanup did not restore {key}')
    if baseline['ride_phase'] == 'running':
        if final['ride_elapsed_ms'] < baseline['ride_elapsed_ms']:
            raise AssertionError('Running ride timeline moved backward during cleanup')
    elif final['ride_elapsed_ms'] != baseline['ride_elapsed_ms']:
        raise AssertionError('Inactive ride elapsed changed during cleanup')
    if final['active'] or final['fake_battery'] or final['x'] != -1 or final['recording']:
        raise AssertionError('Assertion cleanup retained temporary state')


def go_home(device):
    wake_if_dimmed(device)
    state = device.command('STATE')
    for _ in range(3):
        if state['screen'] == 'home':
            return state
        device.command('BUTTON 0 1')
        state = device.command('STATE')
    if state['screen'] == 'home':
        return state
    raise AssertionError(f"Cannot return Home from {state['screen']}")


def scene_change(device, minute):
    go_home(device)
    wake_if_dimmed(device)
    scene = minute % 6
    if scene in (0, 1, 2):
        device.tap(80, 260)
        state = device.command('STATE')
        if state['screen'] != 'ride':
            raise AssertionError(f"Ride scene navigation reached {state['screen']}")
        if scene == 0 and state['ride_phase'] != 'running':
            state = device.command('BUTTON 2 1')
            if state['ride_phase'] != 'running':
                raise AssertionError('Ride scene did not start')
        elif scene == 1:
            old_page = state['ride_page']
            state = device.command('BUTTON 1 1')
            if state['screen'] != 'ride' or state['ride_page'] == old_page:
                raise AssertionError('Ride scene did not change page')
        elif scene == 2 and state['ride_phase'] == 'running':
            state = device.command('BUTTON 2 1')
            if state['ride_phase'] == 'running':
                raise AssertionError('Ride scene did not pause')
    elif scene == 3:
        device.tap(80, 150)
        device.tap(80, 270)
        if device.command('STATE')['screen'] != 'diagnostics':
            raise AssertionError('Diagnostics scene navigation failed')
    elif scene == 4:
        device.tap(80, 100)
        if device.command('STATE')['screen'] != 'settings':
            raise AssertionError('Settings scene navigation failed')
    else:
        device.tap(80, 210)
        if device.command('STATE')['screen'] != 'controls':
            raise AssertionError('Controls scene navigation failed')


def stability(port, output, seconds):
    directory = output / 'stability'
    with Device(port, directory) as device:
        wake_if_dimmed(device)
        initial = device.command('STATE')
        if initial['wifi'] != 0:
            device.expect({'wifi': 4, 'time_status': 'fresh'}, 45)
        baseline = previous = device.command('STATE')
        minimum_heap = baseline['heap_free']
        samples = 0
        started = time.monotonic()
        companion_progress_at = started
        companion_valid = baseline['valid']
        next_scene = started + 60
        next_report = started + 60
        while time.monotonic() - started < seconds:
            device.wait(min(1, seconds - (time.monotonic() - started)))
            current = device.command('STATE')
            samples += 1
            if current['frame'] <= previous['frame']:
                raise AssertionError('Display stopped advancing or device restarted')
            for key in ['bad_crc', 'uart_errors', 'touch_errors']:
                if current[key] != baseline[key]:
                    raise AssertionError(f'{key} increased during stability test')
            minimum_heap = min(minimum_heap, current['heap_free'])
            if minimum_heap < baseline['heap_free'] - 8192:
                raise AssertionError('Transient free heap declined by more than 8 KiB')
            now = time.monotonic()
            if current['valid'] > companion_valid:
                companion_valid = current['valid']
                companion_progress_at = now
            elif now - companion_progress_at >= 5:
                raise AssertionError('Companion valid packet counter stalled for 5 seconds')
            if now >= next_scene:
                scene_change(device, int((now - started) // 60))
                next_scene += 60
            if now >= next_report:
                print(
                    f"stability {int(now - started)}s frame={current['frame']} "
                    f"heap={current['heap_free']} min={minimum_heap} valid={current['valid']}",
                    flush=True,
                )
                next_report += 60
            previous = current
        pre_cleanup = device.command('STATE')
    export(directory)

    time.sleep(1)
    with Device(port, output / 'stability-settled') as device:
        settled = device.command('STATE')
    if settled['heap_free'] < baseline['heap_free'] - 4096:
        raise AssertionError('Post-cleanup free heap retained more than 4 KiB')
    for key in ['bad_crc', 'uart_errors', 'touch_errors']:
        if settled[key] != baseline[key]:
            raise AssertionError(f'{key} increased after cleanup')
    summary = {
        'requested_seconds': seconds,
        'observed_seconds': round(time.monotonic() - started, 3),
        'samples': samples,
        'baseline': baseline,
        'pre_cleanup': pre_cleanup,
        'settled': settled,
        'minimum_heap': minimum_heap,
        'transient_heap_limit_bytes': 8192,
        'retained_heap_limit_bytes': 4096,
        'scene_interval_seconds': 60,
    }
    (output / 'stability-summary.json').write_text(json.dumps(summary, indent=2) + '\n')


def suite(port, output, soak_seconds, skip_persistence):
    for scenario in ['navigation', 'input', 'device-screens', 'diagnostics', 'idle-dimming']:
        debug(port, output, scenario, 'run', ROOT / 'scripts/scenarios' / f'{scenario}.json')
    debug(port, output, 'wifi-clock', 'wifi-recovery')
    debug(port, output, 'ride', 'ride-demo')

    compare(captured_canvas(output, 'ride'), expected_canvas(output, 'ride'))
    actual_diagnostics = captured_canvas(output, 'diagnostics')
    expected_diagnostics = expected_canvas(output, 'diagnostics')
    compare_regions(actual_diagnostics, expected_diagnostics, DIAGNOSTICS_STATIC_REGIONS)
    compare(
        actual_diagnostics,
        expected_diagnostics,
        masks=DIAGNOSTICS_MASKS,
    )
    assertion_cleanup(port, output)
    debug(port, output, 'lease-cleanup', 'lease-test')
    if not skip_persistence:
        run(
            [sys.executable, 'scripts/persistence_test.py', '--port', port,
             '--output', output / 'persistence'],
            output / 'persistence.log',
        )
    stability(port, output, soak_seconds)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', default='/dev/ttyACM0')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--soak-seconds', type=int, default=600)
    parser.add_argument('--skip-persistence', action='store_true')
    parser.add_argument('--only-soak', action='store_true')
    args = parser.parse_args()
    if not 60 <= args.soak_seconds <= 86400:
        raise SystemExit('--soak-seconds must be 60..86400')
    output = args.output or ROOT / '.local/tests' / f'regression-{time.time_ns()}'
    output.mkdir(parents=True, mode=0o700)
    if args.only_soak:
        stability(args.port, output, args.soak_seconds)
    else:
        suite(args.port, output, args.soak_seconds, args.skip_persistence)
    print(f'Regression evidence: {output}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, AssertionError) as error:
        raise SystemExit(str(error)) from None
