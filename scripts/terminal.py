"""One-shot or interactive C606 terminal through the shared no-reset USB owner."""
import argparse
import json
import os
from pathlib import Path
import sys
import time

from usb import ROOT, UsbConnection


def exchange(connection, command):
    try:
        request = f'CMD {connection.request_id + 1} {command}'.encode('ascii')
    except UnicodeEncodeError:
        raise ValueError('commands must be ASCII') from None
    if len(request) > 128 or any(byte in request for byte in (0, 10, 13)):
        raise ValueError('command exceeds the 128-byte line bound or contains a line break')
    reply = connection.terminal_command(command)
    print(json.dumps(reply))
    return reply['status'] in ('OK', 'ACCEPTED')


def run(port, output, command):
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    with UsbConnection(port, output / 'usb.log') as connection:
        if command:
            return 0 if exchange(connection, ' '.join(command)) else 1
        if not exchange(connection, 'INFO'):
            return 1
        print('Enter a command, or quit. USB logs are read while awaiting replies.', file=sys.stderr)
        while True:
            try:
                command = input('cycling> ').strip()
            except EOFError:
                return 0
            if command.lower() in ('quit', 'exit'):
                return 0
            if not command:
                continue
            try:
                exchange(connection, command)
            except ValueError as error:
                print(str(error), file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    parser.add_argument('--output', type=Path,
                        default=ROOT / '.local/terminal' / str(time.time_ns()))
    parser.add_argument('command', nargs='*', help='ordinary command, for example STATUS')
    args = parser.parse_args()
    try:
        return run(args.port, args.output, args.command)
    except KeyboardInterrupt:
        print('\nInterrupted.', file=sys.stderr)
        return 130
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
