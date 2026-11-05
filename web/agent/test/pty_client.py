"""A devtools-shaped pty client driven one command per stdin line.

The open is a copy of cycling_devtools.terminal.usb.open_no_reset, so the
tests exercise the exact termios calls the tools make.

  open | close | write <hex> | read <n> <seconds> | drain <seconds>
"""

import os
import select
import sys
import termios
import time
import tty


def open_no_reset(port):
    fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    try:
        attrs = termios.tcgetattr(fd)
        tty.cfmakeraw(attrs)
        attrs[2] = (attrs[2] | termios.CLOCAL | termios.CREAD) & ~termios.HUPCL
        attrs[4] = attrs[5] = termios.B115200
        termios.tcsetattr(fd, termios.TCSANOW, attrs)
    except BaseException:
        os.close(fd)
        raise
    return fd


def write_all(fd, data):
    while data:
        select.select([], [fd], [], 1.0)
        try:
            data = data[os.write(fd, data):]
        except BlockingIOError:
            pass


def read_for(fd, limit, seconds):
    got = bytearray()
    deadline = time.monotonic() + seconds
    while len(got) < limit:
        left = deadline - time.monotonic()
        if left <= 0:
            break
        ready, _, _ = select.select([fd], [], [], left)
        if ready:
            try:
                got += os.read(fd, 65536)
            except BlockingIOError:
                pass
    return bytes(got)


def main(port):
    fd = None
    for line in sys.stdin:
        verb, *args = line.split()
        if verb == 'open':
            fd = open_no_reset(port)
            reply = 'opened'
        elif verb == 'close':
            os.close(fd)
            fd = None
            reply = 'closed'
        elif verb == 'write':
            write_all(fd, bytes.fromhex(args[0]))
            reply = 'written'
        elif verb == 'read':
            reply = 'got ' + read_for(fd, int(args[0]), float(args[1])).hex()
        elif verb == 'drain':
            reply = f'drained {len(read_for(fd, 1 << 30, float(args[0])))}'
        else:
            reply = f'unknown {verb}'
        print(reply, flush=True)


if __name__ == '__main__':
    main(sys.argv[1])
