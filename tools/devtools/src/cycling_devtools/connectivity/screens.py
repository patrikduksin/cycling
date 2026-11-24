"""Change the main ride page over Wi-Fi as a phone app would, then prove it survives a restart.

    cycling screens-wifi                  # on the home network the device's station is on
    cycling screens-wifi --port /dev/ttyACM0

Over USB it opens a phone session on the home network (`PHONE START HOME`) and reads
the session's address and token. Over HTTP it reads `/api/screens` and the catalogue,
swaps the grid's cadence field for power using the catalogue's ids, dry-runs the tree
(`POST /api/screens/check`), saves it against the revision it read, and checks that a
second save against that revision is refused with 409. It restarts the device
(`RESTART`), opens a new session and reads the page back. At the end it puts the
built-in back (`DELETE /api/screens/ride/main`) and closes the session. A change an
earlier run left is reset first.

This laptop must be on the same network as the device. The report goes to
.local/tests/screens-wifi-<time>/report.json.
"""

import argparse
import json
import re
import time
import urllib.error
import urllib.request
from pathlib import Path

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.harness.base import answer, expect, reopen
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT

SCREEN = 'ride/main'
ADDRESS_TIMEOUT_S = 60
HTTP_TIMEOUT_S = 10


def session(connection):
    """Open a home-network phone session; its base URL and token."""
    expect(connection, 'PHONE START HOME', 'ACCEPTED')
    stop = time.monotonic() + ADDRESS_TIMEOUT_S
    while time.monotonic() < stop:
        data = answer(connection, 'PHONE').get('data', '')
        found = re.search(r'url=(http://[0-9.]+)/\?t=([0-9a-f]+)', data)
        if found:
            return found.group(1), found.group(2)
        if 'state=closed' in data:
            raise AssertionError(f'the phone session closed: {data}')
        time.sleep(1)
    raise TimeoutError('the phone session never had an address; is the station on the home network?')


def call(base, token, method, path, body=None):
    """The status and decoded JSON body of one request."""
    request = urllib.request.Request(base + path, data=body, method=method, headers={'X-Session': token})
    try:
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT_S) as response:
            return response.status, json.loads(response.read() or b'null')
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read() or b'null')


def main_page(screens):
    return next(screen for screen in screens['screens'] if screen['name'] == SCREEN)


def tile(catalogue, name, label):
    found = next(t for t in catalogue['tiles'] if t['tile'] == name and t['label'] == label)
    return found['id'], found['arg']


def edited(screens, catalogue):
    """The main page's nodes with its cadence field showing power."""
    nodes = bytearray.fromhex(main_page(screens)['nodes'])
    leaf, hero = catalogue['kinds']['leaf'], catalogue['kinds']['hero']
    field, cadence = tile(catalogue, 'field', 'CAD RPM')
    _, power = tile(catalogue, 'field', 'PWR 3S W')
    for at in range(0, len(nodes), 4):
        if nodes[at] & ~hero & 0xFF == leaf + field and nodes[at + 1] == cadence:
            nodes[at + 1] = power
            return bytes(nodes)
    raise AssertionError('the main page shows no cadence field to swap')


def check(report, name, condition, detail):
    report['checks'].append({'check': name, 'ok': bool(condition), 'detail': detail})
    if not condition:
        raise AssertionError(f'{name}: {detail}')


def run(port, directory):
    report = {'checks': [], 'ok': False}
    connection = UsbConnection(port, directory / 'usb.bin').__enter__()
    base = token = None
    try:
        base, token = session(connection)
        status, screens = call(base, token, 'GET', '/api/screens')
        check(report, 'screens listed', status == 200, screens)
        revision = screens['revision']
        if main_page(screens)['override']:
            status, reset = call(base, token, 'DELETE', f'/api/screens/{SCREEN}?base={revision}')
            check(report, 'an earlier change reset', status == 200, reset)
            revision = reset['revision']
        status, catalogue = call(base, token, 'GET', '/api/screens/catalogue')
        check(report, 'catalogue listed', status == 200 and catalogue['tiles'], len(catalogue.get('tiles', [])))
        nodes = edited(screens, catalogue)

        status, dry = call(base, token, 'POST', f'/api/screens/check?screen={SCREEN}', nodes)
        check(report, 'dry run accepted', status == 200 and dry['solves'], status)
        status, saved = call(base, token, 'PUT', f'/api/screens/{SCREEN}?base={revision}', nodes)
        check(report, 'saved', status == 200 and saved['revision'] == revision + 1, saved)
        status, stale = call(base, token, 'PUT', f'/api/screens/{SCREEN}?base={revision}', nodes)
        check(report, 'a stale save refused', status == 409 and stale['error'] == 'stale', stale)

        expect(connection, 'RESTART', 'ACCEPTED')
        connection.__exit__(None, None, None)
        connection = None
        time.sleep(2)
        connection = reopen(port, directory, 'restarted', 30)
        base, token = session(connection)
        status, screens = call(base, token, 'GET', '/api/screens')
        main = main_page(screens)
        check(
            report,
            'the change survives a restart',
            status == 200 and main['override'] and bytes.fromhex(main['nodes']) == nodes,
            main,
        )
        status, reset = call(base, token, 'DELETE', f'/api/screens/{SCREEN}?base={screens["revision"]}')
        check(report, 'the built-in restored', status == 200, reset)
        report['ok'] = True
    finally:
        if connection is not None:
            try:
                connection.terminal_command('PHONE STOP')
            finally:
                connection.__exit__(None, None, None)
        (directory / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    add_port_argument(parser)
    parser.add_argument('--output', type=Path, default=ROOT / '.local/tests' / f'screens-wifi-{int(time.time())}')
    args = parser.parse_args()
    port = selected_port(parser, args.port)
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    try:
        return run(port, args.output)
    except (AssertionError, OSError, RuntimeError, TimeoutError) as error:
        print(str(error))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
