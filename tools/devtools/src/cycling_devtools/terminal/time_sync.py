"""Set the C606 clock from this machine's clock, then read it back.

The device takes `TIME SET <unix_seconds>` at a whole second, so the clock
lands within the USB round trip of this machine's. `cycling flash` does this
after it boots a new image. Run it yourself after a power loss, or through
`cycling device time-sync` on the device host. The device keeps the time
through a restart, a crash and sleep; a GNSS fix or Wi-Fi SNTP replaces it.
"""

import argparse
import json
import math
import sys
import time
from pathlib import Path

from cycling_devtools.device.port import add_port_argument, selected_port
from cycling_devtools.terminal.usb import UsbConnection
from cycling_devtools.workspace import ROOT


def command(clock=time.time, sleep=time.sleep):
    now = clock()
    second = math.ceil(now)
    sleep(second - now)
    return f'TIME SET {second}'


def sync(connection, clock=time.time, sleep=time.sleep):
    reply = connection.terminal_command(command(clock, sleep))
    if reply['status'] != 'OK':
        raise RuntimeError(f'TIME SET answered {reply["status"]}: {reply.get("data", "")}')
    return connection.terminal_command('TIME')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    add_port_argument(parser)
    parser.add_argument('--output', type=Path, default=ROOT / '.local/terminal' / str(time.time_ns()))
    args = parser.parse_args()
    port = selected_port(parser, args.port)
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    try:
        with UsbConnection(port, args.output / 'usb.log') as connection:
            print(json.dumps(sync(connection)))
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
