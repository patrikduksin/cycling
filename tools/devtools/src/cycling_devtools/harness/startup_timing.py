"""Measure how fast VANA turns on, connects, starts a ride and answers presses.

VANA records each milestone's device uptime once per boot and reports them with
`RIDE TIMING`, so no record is lost before USB attaches. Uptime starts at the
main-CPU reset; the top-left hold before the companion powers the CPU is not
observable from here.

  --sim      run the simulator scenario `startup_timing` (modelled timings)
  default    wait for the C606 to be turned on, then poll it (CYCLING_SDK=1 build)
  --restart  send RESTART first: a main-CPU restart, the companion keeps running
  --ride     also start a ride with harness presses, pause/resume it and save it
             (records one short ride; needs CYCLING_HARNESS=1). Starts only
             from Home, checks every step and requires the ride to end saved

--restart and --ride refuse while a ride is open on the device.

Evidence and the table go to .local/timing/<time>/.

  --summarize DIR...  median and p90 of each milestone and step over earlier
                      runs (their timing.json), for N repeated measurements
"""

import argparse
import json
import math
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

from cycling_devtools.workspace import ROOT, private_directory

# Field, label, and the milestone the step is measured from.
MILESTONES = (
    ('first_frame_ms', 'Power-on to first frame (backlight on)', None),
    ('home_ms', 'Home accepts input', 'first_frame_ms'),
    ('companion_ms', 'Companion sensors ready', None),
    ('storage_ms', 'Ride storage ready', None),
    ('gnss_fix_ms', 'GNSS fix', None),
    ('ride_press_ms', 'Ride start pressed', None),
    ('recording_ms', 'Recording confirmed', 'ride_press_ms'),
)
SENSOR_NAMES = {
    0: 'BLE heart',
    1: 'BLE cadence',
    11: 'ANT power',
    120: 'ANT heart',
    121: 'ANT speed/cadence',
    122: 'ANT cadence',
    123: 'ANT speed',
    40: 'ANT radar',
}
BUTTON = {'top-left': 0, 'bottom-left': 1, 'bottom-right': 2}
# Home choices in bottom-left order; RIDE is 0.
HOME_CHOICES = 3
IDLE_STATES = ('ready', 'saved', 'recovered')
# VANA ignores a press sooner than 350 ms after the previous one.
PRESS_SETTLE_S = 0.5


def parse(text):
    """`key=value` words from `RIDE TIMING`; `-` is a milestone not reached yet."""
    fields = {}
    for word in text.split():
        key, _, value = word.partition('=')
        if key == 'sensors':
            fields[key] = {}
            if value != '-':
                for pair in value.split(','):
                    kind, _, at = pair.partition(':')
                    fields[key][int(kind)] = int(at)
        elif key:
            fields[key] = None if value == '-' else int(value) if value.lstrip('-').isdigit() else value
    return fields


def table(fields, source):
    rows = [f'| Milestone ({source}) | Uptime ms | Step ms |', '| --- | ---: | ---: |']

    def cell(value):
        return '—' if value is None else str(value)

    for key, label, base in MILESTONES:
        value = fields.get(key)
        step = None if value is None or base is None or fields.get(base) is None else value - fields[base]
        rows.append(f'| {label} | {cell(value)} | {cell(step)} |')
    for kind, at in sorted(fields.get('sensors', {}).items(), key=lambda item: item[1]):
        rows.append(f'| {SENSOR_NAMES.get(kind, f"type {kind}")} live data | {at} | — |')
    if fields.get('storage_state') == 'error':
        rows.append('| Ride storage scan **failed** (state=error) | — | — |')
    if fields.get('sensors_dropped'):
        rows.append(f'| Sensor kinds not recorded (over capacity) | {fields["sensors_dropped"]} | — |')
    rows.append(f'| Press to next frame, last ride press | — | {cell(fields.get("press_frame_last_ms"))} |')
    rows.append(f'| Press to next frame, worst ride press | — | {cell(fields.get("press_frame_max_ms"))} |')
    return '\n'.join(rows)


def percentile(values, fraction):
    """Nearest-rank percentile, so every reported value was observed."""
    ordered = sorted(values)
    return ordered[max(0, math.ceil(fraction * len(ordered)) - 1)]


def summarize(runs):
    """Median and p90 over runs (parsed `fields`) for each milestone and step."""
    rows = [
        f'| Milestone (n={len(runs)}) | Uptime median ms | Uptime p90 ms | Step median ms | Step p90 ms | Runs |',
        '| --- | ---: | ---: | ---: | ---: | ---: |',
    ]

    def stats(values):
        if not values:
            return '—', '—'
        return str(round(statistics.median(values))), str(percentile(values, 0.9))

    named = [(key, label, base) for key, label, base in MILESTONES]
    named += [
        ('press_frame_last_ms', 'Press to next frame, last ride press', None),
        ('press_frame_max_ms', 'Press to next frame, worst ride press', None),
    ]
    for key, label, base in named:
        values = [run[key] for run in runs if isinstance(run.get(key), int)]
        steps = [
            run[key] - run[base]
            for run in runs
            if base and isinstance(run.get(key), int) and isinstance(run.get(base), int)
        ]
        uptime = stats(values) if key.endswith('_ms') and not key.startswith('press_') else ('—', '—')
        step = stats(steps) if base else stats(values) if key.startswith('press_') else ('—', '—')
        rows.append(f'| {label} | {uptime[0]} | {uptime[1]} | {step[0]} | {step[1]} | {len(values)} |')
    kinds = sorted({kind for run in runs for kind in run.get('sensors', {})})
    for kind in kinds:
        values = [run['sensors'][kind] for run in runs if kind in run.get('sensors', {})]
        median, p90 = stats(values)
        name = SENSOR_NAMES.get(kind, f'type {kind}')
        rows.append(f'| {name} live data | {median} | {p90} | — | — | {len(values)} |')
    failed = sum(1 for run in runs if run.get('storage_state') == 'error')
    if failed:
        rows.append(f'| Ride storage scan **failed** | — | — | — | — | {failed} |')
    return '\n'.join(rows)


def write(directory, fields, source, extra=None):
    report = table(fields, source)
    (directory / 'timing.json').write_text(
        json.dumps({'source': source, 'fields': fields} | (extra or {}), indent=2, default=str)
    )
    (directory / 'timing.md').write_text(report + '\n')
    print(report)
    print(f'\nEvidence: {directory}', file=sys.stderr)


def simulate(directory):
    out = directory / 'sim-timing.txt'
    command = [
        'cargo',
        'test',
        '-p',
        'shell-simulator',
        '--locked',
        '--no-default-features',
        '--features',
        'cycling,debug-harness',
        '--test',
        'ride_scenarios',
        'startup_timing',
        '--',
        '--exact',
    ]
    env = dict(os.environ, STARTUP_TIMING_OUT=str(out))
    with (directory / 'cargo.log').open('w') as log:
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
    if result.returncode != 0 or not out.exists():
        print(f'simulator scenario failed; see {directory / "cargo.log"}', file=sys.stderr)
        return 1
    write(directory, parse(out.read_text()), 'simulator, modelled')
    return 0


def wait_for_port(parser, explicit, timeout):
    from cycling_devtools.device.port import DiscoveryError, select_port

    stop = time.monotonic() + timeout
    while True:
        try:
            port = select_port(explicit)
            if Path(port).exists():
                return port
        except DiscoveryError:
            pass
        if time.monotonic() >= stop:
            parser.error('the C606 did not attach over USB in time')
        time.sleep(0.2)


def poll(runner, keys, timeout, sleep=time.sleep):
    """Read `RIDE TIMING` until every key is reached or the wait ends.

    While the companion starts, the device answers BUSY (power work not ready);
    that is a pending reading, retried within the same deadline.
    """
    stop = time.monotonic() + timeout
    fields = {}
    while True:
        reply = runner.transport.terminal_command('RIDE TIMING')
        if reply['status'] == 'OK':
            fields = parse(reply['data'])
            if all(fields.get(key) is not None for key in keys):
                return fields
        elif reply['status'] != 'BUSY':
            raise RuntimeError(f'RIDE TIMING returned {reply["status"]}')
        if time.monotonic() >= stop:
            if not fields:
                raise TimeoutError('device stayed BUSY (power work not ready) for the whole wait')
            return fields
        sleep(0.5)


def require_idle(info):
    """The C606 skill preserves active rides: never press through or restart one.

    `INFO` reports `recording=false` only when no ride is open or being written.
    """
    if info.get('recording') != 'false':
        raise RuntimeError('a ride is open on the device (or its state is unknown); save it on the device first')


def expect(runner, page, state, timeout=10.0, step=0.2):
    """Poll `RIDE STATUS` until VANA shows `page` (any page if None) with the ride in `state`."""
    for _ in range(max(1, int(timeout / step))):
        status = runner.command('RIDE STATUS')
        if (page is None or status.get('page') == page) and status.get('state') == state:
            return status
        runner.delay(step)
    raise RuntimeError(
        f'expected page={page} state={state}, the device shows page={status.get("page")} '
        f'state={status.get("state")}; stopped pressing, check the device'
    )


def ride(runner):
    """Home → RIDE → Ready → start, then pause, resume, end and save (harness presses).

    Presses only from Home with no ride open and no sheet over it, and checks
    the page and ride state after every press. A press can then never edit
    settings or leave a ride recording unnoticed. The ride must end saved.
    """
    runner.command('ACTIVITY')
    status = runner.command('RIDE STATUS')
    if status.get('page') != 'home' or status.get('state') not in IDLE_STATES:
        raise RuntimeError(
            f'--ride starts from Home with no ride open; the device shows page={status.get("page")} '
            f'state={status.get("state")}. Go to Home on the device first.'
        )
    idle, rides = status['state'], int(status['rides'])
    runner.open_session()
    try:

        def press(name, settle=PRESS_SETTLE_S):
            runner.inject({'op': 'input', 'gesture': 'button', 'button': BUTTON[name]})
            runner.delay(settle)

        # Bottom-left on Home only moves the selection: walk it to RIDE.
        for _ in range((HOME_CHOICES - int(status['home_cursor'])) % HOME_CHOICES):
            press('bottom-left')
        if expect(runner, 'home', idle).get('home_cursor') != '0':
            raise RuntimeError('Home did not select RIDE; stopped pressing')
        press('bottom-right')  # RIDE opens Ready
        expect(runner, 'ready', idle)
        for state in ('recording', 'paused', 'recording', 'paused'):
            press('bottom-right')  # start, pause, resume, pause: only a paused ride ends
            expect(runner, 'ride', state)
        press('top-left')  # "End ride?"
        expect(runner, 'end_ride', 'paused')
        press('bottom-right')  # yes: save, then the summary
        saved = expect(runner, 'summary', 'saved')
        if int(saved['rides']) != rides + 1:
            raise RuntimeError(f'expected {rides + 1} saved rides, the device reports {saved["rides"]}')
        press('bottom-right')  # Done
        expect(runner, 'home', 'saved')
    finally:
        if runner.session:
            runner.operation('CLOSE')
            runner.session = None


def device(parser, args, directory):
    from cycling_devtools.harness.runner import Runner
    from cycling_devtools.harness.transport import Real

    if args.restart:
        from cycling_devtools.terminal.usb import reply_fields

        port = wait_for_port(parser, args.port, 5)
        with Real(port, directory / 'restart-usb.log') as transport:
            reply = transport.terminal_command('INFO')
            require_idle(reply_fields(reply['data']) if reply['status'] == 'OK' else {})
            transport.terminal_command('RESTART')
        time.sleep(1.0)
    else:
        print('Turn the C606 on now (hold the top-left button).', file=sys.stderr)
    port = wait_for_port(parser, args.port, args.seconds)
    with Real(port, directory / 'usb.log') as transport:
        runner = Runner(transport, directory)
        try:
            info = runner.command('INFO')
            fields = poll(runner, ('first_frame_ms', 'home_ms', 'storage_ms'), args.seconds)
            if args.ride:
                # Asked again now: a ride may have started since power-on.
                require_idle(runner.command('INFO'))
                ride(runner)
            keys = ['gnss_fix_ms'] if args.gnss else []
            fields = poll(runner, keys, args.seconds if keys else 0)
        finally:
            runner.trace.close()
    write(directory, fields, 'C606', {'info': info, 'restart': args.restart, 'ride': args.ride})
    return 0


def main():
    from cycling_devtools.device.port import add_port_argument

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--sim', action='store_true', help='measure the simulator scenario instead of the device')
    parser.add_argument('--restart', action='store_true', help='send RESTART instead of waiting for power-on')
    parser.add_argument('--ride', action='store_true', help='start, pause, resume and save one short ride')
    parser.add_argument('--gnss', action='store_true', help='also wait for a GNSS fix (needs sky view)')
    parser.add_argument('--seconds', type=float, default=120, help='bound on each wait')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--summarize', type=Path, nargs='+', metavar='DIR', help='summarize earlier runs')
    add_port_argument(parser)
    args = parser.parse_args()
    if args.summarize:
        runs = []
        for run in args.summarize:
            fields = json.loads((run / 'timing.json').read_text())['fields']
            fields['sensors'] = {int(kind): at for kind, at in fields.get('sensors', {}).items()}
            runs.append(fields)
        print(summarize(runs))
        return 0
    directory = private_directory(args.output or ROOT / '.local/timing' / time.strftime('%Y%m%d-%H%M%S'))
    try:
        return simulate(directory) if args.sim else device(parser, args, directory)
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        print(json.dumps({'status': 'fail', 'error': str(error), 'evidence': str(directory)}))
        return 1


if __name__ == '__main__':
    sys.exit(main())
