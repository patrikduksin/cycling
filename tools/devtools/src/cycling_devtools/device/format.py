"""Report what each of VANA's stores holds, or format them all.

    cycling format            every store: current, blank, foreign or unusable
    cycling format --confirm  erase the rides, the traces, the settings and the files, then wait for the restart

A store written by another firmware version reads `foreign`: the device keeps it and offers FORMAT on its
screen at boot. `--confirm` runs the same wipe from here, foreign data or not. The terrain pack is kept
unless the volume table is not one this version reads, or the pack itself is of another version.
"""

import argparse
import json
import time

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.harness.base import reopen
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT, private_path

RESTART_S = 120


def health(connection):
    reply = connection.terminal_command('FORMAT')
    if reply['status'] != 'OK':
        raise RuntimeError(f'FORMAT: {reply["status"]} {reply["data"]}')
    return dict(field.split('=', 1) for field in reply['data'].split())


def wait_for_restart(connection, sleep):
    try:
        while (state := health(connection)['state']) == 'running':
            sleep(0.5)
    except OSError, TimeoutError:
        return 'done'
    return state


def wipe(port, log_dir, sleep=time.sleep):
    with UsbConnection(port, log_dir / f'usb-{time.time_ns()}.log') as connection:
        reply = connection.terminal_command('FORMAT CONFIRM')
        if reply['status'] != 'ACCEPTED':
            raise RuntimeError(f'FORMAT CONFIRM: {reply["status"]} {reply["data"]}')
        state = wait_for_restart(connection, sleep)
    if state == 'failed':
        raise RuntimeError('the wipe failed; power the device off and on, then format again')
    connection = reopen(port, log_dir, 'after-format', RESTART_S)
    try:
        after = health(connection)
    finally:
        connection.__exit__(None, None, None)
    left = sorted(name for name, value in after.items() if value in ('foreign', 'unread'))
    if left:
        raise RuntimeError(f'still not formatted after the restart: {", ".join(left)}')
    return after


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    add_port_argument(parser)
    parser.add_argument(
        '--confirm', action='store_true', help='erase the rides, the traces, the settings and the files'
    )
    args = parser.parse_args(argv)
    port = selected_port(parser, args.port)
    log_dir = private_path(ROOT / '.local' / 'volumes')
    log_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    if args.confirm:
        result = wipe(port, log_dir)
    else:
        with UsbConnection(port, log_dir / f'usb-{time.time_ns()}.log') as connection:
            result = health(connection)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        raise SystemExit(str(error)) from None
