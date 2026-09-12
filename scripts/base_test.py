"""Check the ordinary terminal; mutation/restart checks require explicit flags."""
import argparse
import json
import re
import time
from pathlib import Path

from usb import UsbConnection
from service_stress import PREFERENCES, private_directory, read, validate


def expect(connection, command, status):
    if status == 'INVALID':
        # Parser failures deliberately use id0; only one request is outstanding.
        connection.request_id = -1
    reply = connection.terminal_command(command)
    if reply['status'] != status:
        raise AssertionError(f'{command}: expected {status}, got {reply["status"]}')
    return reply


def reopen(port, directory, label, deadline=20):
    """Retry only opening a new read-only session, never replay a prior command."""
    stop = time.monotonic() + deadline
    attempt = 0
    while True:
        attempt += 1
        connection = UsbConnection(port, directory / f'{label}-{attempt}.bin')
        entered = False
        try:
            connection.__enter__()
            entered = True
            read(connection, 'INFO')
            return connection
        except (OSError, RuntimeError, TimeoutError):
            if entered:
                connection.__exit__(None, None, None)
            if time.monotonic() >= stop:
                raise
            time.sleep(.25)


def wifi_failures(connection):
    reply = expect(connection, 'WIFI', 'OK')
    match = re.search(r'stats=\((\d+),\s*(\d+),\s*(\d+),\s*(\d+)\)', reply['data'])
    if match is None:
        raise AssertionError('missing Wi-Fi recovery counters')
    return int(match.group(3))


def preserved(expected, actual):
    for key in PREFERENCES:
        if actual[key] != expected[key]:
            raise AssertionError(f'preference mismatch: {key}')


def apply(connection, settings):
    expect(connection, f'BRIGHTNESS {settings["brightness"]}', 'OK')
    expect(connection, f'IDLE {settings["dim_timeout"]} {settings["dim_brightness"]}', 'OK')
    expect(connection, f'TIMEZONE {settings["timezone"]}', 'OK')
    # A timeout here has unknown completion. Caller must not retry this save.
    expect(connection, 'SAVE', 'OK')


def transport_recovery(connection, build, result):
    """One controlled stall, no retry. Loss counters are evidence, not a target."""
    if build.get('cycling') != 'false':
        raise ValueError('transport recovery is restricted to base firmware without rides')
    if build.get('harness') != 'true':
        expect(connection, 'TEST 20', 'UNSUPPORTED')
        result.update(supported=False)
        return
    result.update(supported=True, recovered=False, recording=False)
    before_position = read(connection, 'POSITION')
    before_input = read(connection, 'INPUT')
    settings = read(connection, 'SETTINGS')
    system = read(connection, 'STATUS')
    result.update(before_position=before_position, before_input=before_input,
                  before_status=system)
    # Firmware replies after its six-second stall. The larger deadline is for
    # this single command only; a timeout never causes another stall request.
    ack = connection.terminal_command('TEST 20', timeout=12)
    result['ack'] = ack
    if ack['status'] != 'ACCEPTED':
        raise AssertionError('controlled transport stall was not accepted')
    result['post_stall_position'] = read(connection, 'POSITION')
    result['post_stall_input'] = read(connection, 'INPUT')
    deadline = time.monotonic() + 15
    while True:
        position = read(connection, 'POSITION')
        inputs = read(connection, 'INPUT')
        result.update(after_position=position, after_input=inputs)
        if (position.get('transport') == 'receiving'
                and int(position['valid']) > int(before_position['valid'])
                and int(inputs['companion_valid']) > int(before_input['companion_valid'])):
            break
        if time.monotonic() >= deadline:
            raise TimeoutError('transport/parser acquisition did not recover after stall')
        time.sleep(.25)
    # Observe another receive window instead of accepting a single transient
    # recovery snapshot. An indoor no-fix report is valid transport progression.
    time.sleep(2)
    settled_position = read(connection, 'POSITION')
    settled_input = read(connection, 'INPUT')
    result.update(settled_position=settled_position, settled_input=settled_input)
    if settled_position.get('transport') != 'receiving':
        raise AssertionError('position transport did not remain receiving')
    result['position_delta'] = validate('POSITION', before_position, settled_position, True)
    result['input_delta'] = validate('INPUT', before_input, settled_input, True)
    validate('POSITION', position, settled_position, True)
    validate('INPUT', inputs, settled_input, True)
    preserved(settings, read(connection, 'SETTINGS'))
    if read(connection, 'STATUS')['storage_ops'] != system['storage_ops']:
        raise AssertionError('storage operation during transport-only test')
    result['recovered'] = True


def run(args):
    directory = private_directory(args.output)
    connection = reopen(args.port, directory, 'initial')
    try:
        original = read(connection, 'SETTINGS')
        initial_system = read(connection, 'STATUS')
        build = read(connection, 'INFO')
    except BaseException:
        connection.__exit__(None, None, None)
        raise
    changed = False
    restore_uncertain = False
    report = dict(build=build, recording=False, ok=False)
    try:
        for command in ('BRIGHTNESS 101', 'TIMEZONE 841', 'IDLE 3601 20', 'SAVE extra'):
            expect(connection, command, 'INVALID')
        # Malformed/overlong commands must not execute a suffix or reset the board.
        overlong = expect(connection, 'INVALID ' + 'x' * 128 + ' RESTART', 'INVALID')
        if '128' not in overlong['data']:
            raise AssertionError('expected explicit overlong rejection')
        read(connection, 'STATUS')
        position = read(connection, 'POSITION')
        inputs = read(connection, 'INPUT')
        baseline = read(connection, 'STATUS')
        time.sleep(args.seconds)
        report['position_delta'] = validate('POSITION', position, read(connection, 'POSITION'))
        report['input_delta'] = validate('INPUT', inputs, read(connection, 'INPUT'))
        after = read(connection, 'STATUS')
        if after['display_submissions'] != baseline['display_submissions']:
            raise AssertionError('display submitted during no-render acquisition window')
        if after['storage_ops'] != baseline['storage_ops']:
            raise AssertionError('storage operation during read-only acquisition window')
        connection.__exit__(None, None, None)
        connection = None
        for index in range(3):
            connection = reopen(args.port, directory, f'reopen-{index}')
            system = read(connection, 'STATUS')
            if int(system['uptime_ms']) < int(after['uptime_ms']):
                raise AssertionError('ordinary USB open reset device')
            preserved(original, read(connection, 'SETTINGS'))
            after = system
            connection.__exit__(None, None, None)
            connection = None
        connection = reopen(args.port, directory, 'options')
        if args.transport_recovery:
            report['transport_recovery'] = {}
            transport_recovery(connection, build, report['transport_recovery'])
        if args.wifi_recovery:
            if build.get('harness') != 'true':
                expect(connection, 'TEST 2', 'UNSUPPORTED')
            else:
                failures_before = wifi_failures(connection)
                expect(connection, 'TEST 2', 'ACCEPTED')
                try:
                    expect(connection, 'WIFI RECONNECT', 'ACCEPTED')
                    deadline = time.monotonic() + 60
                    while wifi_failures(connection) <= failures_before:
                        if time.monotonic() >= deadline:
                            raise TimeoutError('injected Wi-Fi failure was not observed')
                        time.sleep(.5)
                finally:
                    expect(connection, 'TEST 0', 'ACCEPTED')
                expect(connection, 'WIFI RECONNECT', 'ACCEPTED')
                deadline = time.monotonic() + 90
                while read(connection, 'WIFI')['state'] != '4':
                    if time.monotonic() >= deadline:
                        raise TimeoutError('Wi-Fi public probe did not recover')
                    time.sleep(.5)
        if args.panic:
            if build.get('harness') != 'true':
                expect(connection, 'TEST 11', 'UNSUPPORTED')
            else:
                expect(connection, 'TEST 11', 'ACCEPTED')
                connection.__exit__(None, None, None)
                connection = None
                time.sleep(2)
                connection = reopen(args.port, directory, 'panic', 30)
                crash = read(connection, 'STATUS')
                if crash['crash'] != 'controlled' or crash['reset'] != 'software':
                    raise AssertionError('controlled crash marker not observed')
                expect(connection, 'RESTART', 'ACCEPTED')
                connection.__exit__(None, None, None)
                connection = None
                time.sleep(2)
                connection = reopen(args.port, directory, 'panic-clean', 30)
                if read(connection, 'STATUS')['crash'] != 'none':
                    raise AssertionError('crash marker replayed after clean restart')
                preserved(original, read(connection, 'SETTINGS'))
        if args.write_settings:
            changed = True
            temporary = dict(original, brightness='5' if original['brightness'] != '5' else '10')
            apply(connection, temporary)
        else:
            temporary = original
        if args.restart or args.write_settings:
            expect(connection, 'RESTART', 'ACCEPTED')
            connection.__exit__(None, None, None)
            connection = None
            time.sleep(2)
            connection = reopen(args.port, directory, 'restart', 30)
            preserved(temporary, read(connection, 'SETTINGS'))
            restarted = read(connection, 'STATUS')
            if restarted['reset'] != 'software':
                raise AssertionError('expected software reset')
        report['ok'] = True
    finally:
        try:
            if changed:
                # Restore with a new verified settings operation. Never repeat an
                # ambiguous test SAVE or an ambiguous restore SAVE blindly.
                try:
                    if connection is None:
                        connection = reopen(args.port, directory, 'restore', 30)
                    read(connection, 'SETTINGS')
                    apply(connection, original)
                    preserved(original, read(connection, 'SETTINGS'))
                except BaseException:
                    restore_uncertain = True
                    report['ok'] = False
                    raise
                finally:
                    report['restore_uncertain'] = restore_uncertain
        finally:
            if connection is not None:
                connection.__exit__(None, None, None)
            report['initial_uptime_ms'] = initial_system['uptime_ms']
            (directory / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--port', default='/dev/ttyACM0')
    parser.add_argument('--seconds', type=float, default=5)
    parser.add_argument('--restart', action='store_true')
    parser.add_argument('--write-settings', action='store_true')
    parser.add_argument('--wifi-recovery', action='store_true')
    parser.add_argument('--panic', action='store_true')
    parser.add_argument('--transport-recovery', action='store_true',
                        help='Base firmware only: one six-second executor stall and bounded recovery check')
    args = parser.parse_args()
    if not 2 <= args.seconds <= 120:
        parser.error('--seconds must be 2..120')
    print(json.dumps(run(args), indent=2))

if __name__ == '__main__':
    main()
