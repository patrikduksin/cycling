"""Measure button-press-to-frame latency and idle frames on a C606 harness build.

Each trial schedules one synthetic press at a random phase, so it can fall due
while a frame is rendering. The firmware times each routed press from when it
fell due (physical presses: when they were received) to the end of the next
frame submission and reports it in STATUS as press_frame_ms. Before the
presses, an idle window gives the frame rate (display_submissions per second)
and the last idle frame's render-and-transfer time (display_ms), with no
capture running. The idle window can outlast the dim timeout, and a press on a
dimmed screen only wakes it, so the tool refreshes activity before timing and
before every press. Requires a CYCLING_HARNESS=1 C606 build; the simulator has no
wall clock to time.
"""

import argparse
import json
import random
import statistics
import sys
import time

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.harness.runner import Runner
from cycling_devtools.harness.screenshot import button
from cycling_devtools.harness.transport import Real
from cycling_devtools.workspace import ROOT, private_path

# VANA ignores presses within 350 ms of the previous one.
PHASE_MS = (400, 900)


def idle_frames(runner, seconds):
    """Frames per second over an idle window, and the last frame's milliseconds."""
    before = runner.command('STATUS')
    runner.delay(seconds)
    after = runner.command('STATUS')
    frames = int(after['display_submissions']) - int(before['display_submissions'])
    elapsed_ms = int(after['uptime_ms']) - int(before['uptime_ms'])
    rate = round(frames * 1000 / elapsed_ms, 2) if elapsed_ms > 0 else None
    return rate, int(after['display_ms'])


def wake(runner, timeout_s=2):
    """Refresh activity, as a press would, and wait for the next frame so a
    dimmed screen is lit again before anything is timed."""
    before = int(runner.command('STATUS')['display_submissions'])
    runner.command('ACTIVITY')
    stop = time.monotonic() + timeout_s
    while time.monotonic() < stop:
        if int(runner.command('STATUS')['display_submissions']) > before:
            return
        runner.delay(0.05)


def press_once(runner, index, phase_ms, timeout_s=5):
    # Keep the screen awake: a press that only wakes it draws no timed frame.
    runner.command('ACTIVITY')
    before = int(runner.command('STATUS')['press_frames'])
    runner.inject({'events': [{'at_ms': phase_ms, 'event': f'BUTTON {index}'}]})
    stop = time.monotonic() + timeout_s
    while time.monotonic() < stop:
        status = runner.command('STATUS')
        if int(status['press_frames']) > before:
            return int(status['press_frame_ms'])
        runner.delay(0.05)
    raise TimeoutError('no frame followed the press')


def measure(runner, index, count, setup=(), idle_s=5, seed=None):
    caps = runner.command('HARNESS CAPS')
    if caps.get('input') != 'supported':
        raise RuntimeError(f'synthetic input is {caps.get("input", "unsupported")}; flash a CYCLING_HARNESS=1 build')
    status = runner.command('STATUS')
    if 'press_frames' not in status or 'display_ms' not in status:
        raise RuntimeError('firmware does not report press_frame_ms and display_ms')
    rng = random.Random(seed)
    runner.open_session()
    try:
        runner.command('ACTIVITY')
        runner.delay(0.5)
        for step in setup:
            runner.inject({'op': 'input', 'gesture': 'button', 'button': step})
            runner.delay(0.5)
        rate, frame_ms = idle_frames(runner, idle_s)
        wake(runner)
        samples = [press_once(runner, index, rng.randrange(*PHASE_MS)) for _ in range(count)]
    finally:
        if runner.session:
            runner.operation('CLOSE')
            runner.session = None
    ordered = sorted(samples)
    return {
        'status': 'pass',
        'button': index,
        'presses': count,
        'press_frame_ms': samples,
        'median_ms': statistics.median(ordered),
        'p90_ms': ordered[min(len(ordered) - 1, (len(ordered) * 9) // 10)],
        'max_ms': ordered[-1],
        'idle_frames_per_s': rate,
        'idle_frame_ms': frame_ms,
        'physical_panel_verified': False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--press', type=button, default=0, help='button to time (default top-left)')
    parser.add_argument('--count', type=int, default=20, help='timed presses')
    parser.add_argument(
        '--setup',
        type=button,
        action='append',
        default=[],
        help='untimed presses first, repeatable, in order (navigation)',
    )
    parser.add_argument('--idle-s', type=float, default=5, help='idle window for the frame rate')
    parser.add_argument('--seed', type=int, help='press phase seed')
    add_port_argument(parser)
    args = parser.parse_args()
    if not 1 <= args.count <= 200:
        parser.error('--count must be 1..200')
    stamp = time.strftime('%Y%m%d-%H%M%S')
    evidence = private_path(ROOT / '.local/latency' / f'{stamp}-{time.time_ns() % 1_000_000:06}')
    evidence.mkdir(parents=True, mode=0o700)
    transport = Real(selected_port(parser, args.port), evidence / 'usb.log')
    try:
        with transport:
            runner = Runner(transport, evidence)
            try:
                result = measure(runner, args.press, args.count, args.setup, args.idle_s, args.seed)
            finally:
                runner.trace.close()
    except (OSError, ValueError, RuntimeError, TimeoutError) as error:
        print(json.dumps({'status': 'fail', 'error': str(error), 'evidence': str(evidence)}))
        return 1
    print(json.dumps(result | {'evidence': str(evidence)}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
