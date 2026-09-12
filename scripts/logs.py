"""Private, reconnecting C606 JSON Lines capture through the single USB owner."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import select
import time

from usb import ROOT, acquire_lock, open_no_reset
MAX_LINE = 4096


class Decoder:
    def __init__(self, emit):
        self.emit = emit
        self.pending = bytearray()
        self.discard = False
        self.boot = None
        self.seq = None
        self.counts = Counter()

    def host(self, event, **fields):
        self.counts[event] += 1
        self.emit(dict(type='host', event=event, host_ns=time.time_ns(), **fields))

    def boundary(self, event, **fields):
        if self.pending or self.discard:
            self.host('truncated', bytes=len(self.pending))
        self.pending.clear()
        self.discard = False
        self.host(event, **fields)

    def feed(self, data):
        for byte in data:
            if byte == 10:
                if not self.discard:
                    self.line(bytes(self.pending))
                self.pending.clear()
                self.discard = False
            elif not self.discard:
                if len(self.pending) == MAX_LINE:
                    self.host('overlong', limit=MAX_LINE)
                    self.pending.clear()
                    self.discard = True
                else:
                    self.pending.append(byte)

    def line(self, raw):
        if not raw.strip():
            return
        try:
            record = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            # Preserve panic, ROM and legacy text in raw.bin; mark invalid JSON.
            self.host('unstructured', bytes=len(raw))
            return
        if not isinstance(record, dict) or record.get('type') not in ('log', 'reply'):
            self.host('unrecognized')
            return
        if record['type'] == 'log':
            boot, seq = record.get('boot'), record.get('seq')
            if (type(boot) is not int or not 0 <= boot <= 0xffffffff
                    or type(seq) is not int or not 0 <= seq <= 0xffffffff):
                self.host('malformed_log')
                return
            if boot != self.boot:
                self.host('boot_boundary', boot=boot, initial=self.boot is None)
                self.boot, self.seq = boot, None
            if self.seq is not None and seq != (self.seq + 1) & 0xffffffff:
                self.host('sequence_gap', boot=boot, previous=self.seq, current=seq,
                          missing=(seq - self.seq - 1) & 0xffffffff)
            self.seq = seq
            self.counts['logs'] += 1
            if record.get('level') in ('ERROR', 'WARN'):
                self.counts['errors_warnings'] += 1
        else:
            self.counts['replies'] += 1
        self.emit(dict(record, host_ns=time.time_ns()))


def private_file(path, mode):
    return os.fdopen(os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), mode)


def collect(port, directory, seconds=None, commands=()):
    directory = directory.resolve()
    if not directory.is_relative_to((ROOT / '.local').resolve()):
        raise ValueError('Capture output must be inside ignored .local/')
    directory.mkdir(parents=True, exist_ok=False, mode=0o700)
    (ROOT / '.local').mkdir(exist_ok=True)
    with acquire_lock():
        with private_file(directory / 'raw.bin', 'wb') as raw, \
                private_file(directory / 'records.jsonl', 'w') as decoded:
            def emit(record):
                decoded.write(json.dumps(record, separators=(',', ':')) + '\n')
                decoded.flush()
            decoder = Decoder(emit)
            decoder.host('session_start', port=port, commands=list(commands))
            deadline = None if seconds is None else time.monotonic() + seconds
            fd = None
            retry = .1
            outgoing = b''
            first_connection = True
            try:
                while deadline is None or time.monotonic() < deadline:
                    if fd is None:
                        try:
                            fd = open_no_reset(port)
                        except OSError as error:
                            decoder.host('open_retry', errno=error.errno, delay=retry)
                            time.sleep(retry)
                            retry = min(2., retry * 2)
                            continue
                        decoder.boundary('connected')
                        retry = .1
                        outgoing = b"CMD 0 INFO\n"
                        # Commands execute once only. Reconnect must never replay mutations.
                        if first_connection:
                            outgoing += b''.join(f'CMD {i} {cmd}\n'.encode('ascii')
                                                for i, cmd in enumerate(commands, 1))
                            first_connection = False
                    try:
                        ready, writable, _ = select.select([fd], [fd] if outgoing else [], [], .1)
                        if writable:
                            try:
                                outgoing = outgoing[os.write(fd, outgoing):]
                            except BlockingIOError:
                                pass
                        data = os.read(fd, 16384) if ready else None
                        if data == b'':
                            raise OSError('USB disconnected')
                    except BlockingIOError:
                        continue
                    except OSError as error:
                        decoder.boundary('disconnected', error=str(error), unsent_bytes=len(outgoing))
                        outgoing = b''
                        os.close(fd)
                        fd = None
                        continue
                    if data:
                        # Disk failures propagate; never masquerade as a serial retry.
                        raw.write(data)
                        raw.flush()
                        decoder.feed(data)
            except KeyboardInterrupt:
                pass
            finally:
                if fd is not None:
                    os.close(fd)
                decoder.boundary('session_end')
                with private_file(directory / 'summary.json', 'w') as report:
                    json.dump(dict(decoder.counts), report, indent=2)
    return directory


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--port', default=os.environ.get('CYCLING_PORT', '/dev/ttyACM0'))
    p.add_argument('--output', type=Path)
    p.add_argument('--seconds', type=float)
    p.add_argument('--command', action='append', default=[], help='Send once as CMD <id> <text>; never replay after reconnect')
    p.add_argument('--filter', type=Path, help='Read saved records instead of opening USB')
    p.add_argument('--component')
    p.add_argument('--level')
    args = p.parse_args()
    if args.filter:
        with args.filter.open() as records:
            for line in records:
                r = json.loads(line)
                if ((not args.component or r.get('component') == args.component)
                        and (not args.level or r.get('level') == args.level)):
                    print(json.dumps(r))
        return
    if args.seconds is not None and args.seconds <= 0:
        p.error('--seconds must be positive')
    for cmd in args.command:
        if not cmd or len(cmd) > 160 or not cmd.isascii() or any(ord(c) < 32 for c in cmd):
            p.error('commands must be 1..160 printable ASCII characters')
    print(collect(args.port, args.output or ROOT / '.local/logs' / str(time.time_ns()),
                  args.seconds, args.command))


if __name__ == '__main__':
    main()
