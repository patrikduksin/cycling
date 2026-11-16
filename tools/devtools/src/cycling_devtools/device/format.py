"""Report what each of VANA's stores holds, or format them all.

    cycling format            every store: current, blank, foreign or unusable
    cycling format --confirm  erase the rides, the traces, the files and the settings, then wait for the restart

A store written by another firmware version reads `foreign`: the device keeps it and offers FORMAT on its
screen at boot. `--confirm` runs the same wipe from here, foreign data or not. It keeps the terrain pack
under a volume table of this version, unless the pack itself is of another version; any other table is
replaced, and the pack goes with it.
"""

import argparse
import contextlib
import json
import time

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.harness.base import poll_across_resets
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT, private_path

RESTART_S = 120


def health(connection):
    reply = connection.terminal_command('FORMAT')
    if reply['status'] != 'OK':
        raise RuntimeError(f'FORMAT: {reply["status"]} {reply["data"]}')
    return dict(field.split('=', 1) for field in reply['data'].split())


def finished(connection):
    """Done once the wipe failed, or the restarted device has read every store."""
    stores = health(connection)
    state = stores['state']
    settled = state not in ('running', 'done') and 'unread' not in stores.values()
    return state == 'failed' or settled, stores


def wipe(port, log_dir):
    with contextlib.ExitStack() as opened:
        connection = opened.enter_context(UsbConnection(port, log_dir / f'usb-{time.time_ns()}.log'))
        reply = connection.terminal_command('FORMAT CONFIRM')
        if reply['status'] != 'ACCEPTED':
            raise RuntimeError(f'FORMAT CONFIRM: {reply["status"]} {reply["data"]}')
        connection, after = poll_across_resets(connection, port, log_dir, 'format', RESTART_S, finished)
        opened.push(connection)
    if after['state'] == 'failed':
        raise RuntimeError('the wipe failed; power the device off and on, then format again')
    left = sorted(name for name, value in after.items() if value == 'foreign')
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
