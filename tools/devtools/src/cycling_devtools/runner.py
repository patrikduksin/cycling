"""Runs gates as shell commands with a log each, and can stop every one of them."""

import contextlib
import os
import signal
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools.lanes import Gate


@dataclass
class Result:
    gate: Gate
    ok: bool
    seconds: float
    log: Path | None = None
    cached: bool = False


class Gates:
    def __init__(self, cwd, logs):
        self.cwd = cwd
        self.logs = logs
        self.lock = threading.Lock()
        self.running = set()
        self.stopped = False

    def run(self, gate, command=None):
        command = command or gate.command
        log = self.logs / f'{gate.name}.log'
        started = time.monotonic()
        with log.open('w') as out:
            out.write(f'$ {command}\n')
            out.flush()
            with self.lock:
                if self.stopped:
                    return Result(gate, False, 0.0, log)
                process = subprocess.Popen(
                    command,
                    shell=True,
                    cwd=self.cwd,
                    env=os.environ | dict(gate.env),
                    stdout=out,
                    stderr=subprocess.STDOUT,
                    process_group=0,
                )
                self.running.add(process)
            code = process.wait()
            with self.lock:
                self.running.discard(process)
        return Result(gate, code == 0, time.monotonic() - started, log)

    def stop(self):
        with self.lock:
            self.stopped = True
            for process in self.running:
                # Each gate leads its own process group, so this reaches the compilers and tests below the shell.
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGTERM)


def stop_on_signals():
    """A terminated run unwinds like an interrupted one, so it stops its gates on the way out."""

    def stop(number, frame):
        raise SystemExit(128 + number)

    for number in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, stop)


def failure_tail(results, lines=30):
    parts = []
    for result in results:
        if not result.ok:
            tail = result.log.read_text(errors='replace').splitlines()[-lines:]
            parts.append(f'--- {result.gate.name}\n' + '\n'.join(tail))
    return '\n'.join(parts)


def children_cpu():
    times = os.times()
    return times.children_user + times.children_system
