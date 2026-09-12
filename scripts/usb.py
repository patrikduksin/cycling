"""Exclusive no-reset USB access and single-request terminal reply matching.

A command is sent once. Reboots, disconnects and timeouts leave mutations uncertain;
callers must inspect state instead of replaying them. Log decoding stays in logs.py.
"""
import fcntl
import json
import os
from pathlib import Path
import select
import termios
import time
import tty

ROOT = Path(__file__).resolve().parents[1]


def acquire_lock():
    (ROOT / '.local').mkdir(exist_ok=True)
    lock = (ROOT / '.local/usb.lock').open('a')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BaseException:
        lock.close()
        raise
    return lock


def open_no_reset(port):
    """Linux tty open without modem-control ioctls, reset, or input flushing."""
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


class UsbConnection:
    def __init__(self, port, log):
        self.port, self.log_path = port, Path(log)
        self.fd = self.lock = self.log = None
        self.pending = bytearray()
        self.request_id = 0
        self.boot = [None]

    def __enter__(self):
        try:
            self.lock = acquire_lock()
            self.fd = open_no_reset(self.port)
            self.log = self.log_path.open('wb')
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_):
        fd, log, lock = self.fd, self.log, self.lock
        self.fd = self.log = self.lock = None
        try:
            if fd is not None:
                os.close(fd)
        finally:
            try:
                if log is not None:
                    log.close()
            finally:
                if lock is not None:
                    lock.close()

    def terminal_command(self, command, timeout=8):
        """Send once. The caller decides how to handle an uncertain mutation."""
        self.request_id += 1
        request = f'CMD {self.request_id} {command}\n'.encode()
        if os.write(self.fd, request) != len(request):
            raise RuntimeError('short USB command write; outcome may be uncertain')
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not select.select([self.fd], [], [], min(.2, max(0, deadline - time.monotonic())))[0]:
                continue
            data = os.read(self.fd, 65536)
            if not data:
                raise RuntimeError('device disconnected; command outcome may be uncertain')
            self.log.write(data)
            self.log.flush()
            reply = terminal_reply(self.pending, data, self.request_id, self.boot)
            if reply is not None:
                return reply
        raise TimeoutError(f'no terminal response for {command}')


def terminal_reply(pending, data, expected_id, boot=None):
    pending.extend(data)
    while b'\n' in pending:
        line, _, remainder = pending.partition(b'\n')
        pending[:] = remainder
        if len(line) > 4096:
            raise ValueError('oversized USB terminal line')
        if b'CYCLING_BOOT ' in line:
            raise RuntimeError('device rebooted during command')
        try:
            reply = json.loads(line)
        except (UnicodeDecodeError, ValueError):
            continue
        if not isinstance(reply, dict):
            continue
        if reply.get('type') == 'log':
            # The first observed integer token establishes this attachment's
            # identity, even when earlier startup text was unstructured. Reply
            # priority can also delay startup logs until after the first reply.
            if type(reply.get('boot')) is int:
                if boot is not None:
                    if boot[0] is not None and boot[0] != reply['boot']:
                        raise RuntimeError('device rebooted during command')
                    boot[0] = reply['boot']
            elif reply.get('component') == 'boot':
                raise RuntimeError('boot record has no identity; command outcome is uncertain')
            continue
        if reply.get('type') != 'reply':
            continue
        # The firmware discards the id when line parsing fails. This connection
        # permits only one outstanding command, so its documented id-zero parse
        # rejection belongs to that command. Never accept id-zero success or an
        # unrelated numbered reply as its completion.
        parse_rejection = (type(reply.get('id')) is int and reply['id'] == 0
                           and reply.get('status') in ('INVALID', 'OVERLONG'))
        if reply.get('id') != expected_id and not parse_rejection:
            continue
        if (type(reply.get('id')) is not int
                or not isinstance(reply.get('status'), str)
                or not isinstance(reply.get('data'), str)):
            raise ValueError('malformed terminal reply')
        return reply
    if len(pending) > 4096:
        raise ValueError('oversized USB terminal line')
    return None
