"""cycling <task> [args...]: every development task, run from the repository root.

The dev shell puts `cycling` on PATH (nix/shell.nix) and starts it in the locked
tools/devtools environment, so devtools steps call `python -m` directly. A task
is shell steps run in order; arguments reach the last one as "$@" unless a step
names them itself. `needs` run first, side by side, each line prefixed.
"""

import os
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass

from cycling_devtools.workspace import ROOT

DEV = 'python -m cycling_devtools.'
# The terrain pack builder has its own environment (osmium, numpy, tifffile).
TERRAIN = 'uv run --project tools/terrain --locked'
# BlueZ tools need the distribution's D-Bus and GLib bindings, which only the
# system Python provides; they use the source tree instead of the uv environment.
SYSTEM_PYTHON = 'PYTHONPATH=tools/devtools/src /usr/bin/python3 -m cycling_devtools.'
SIM_SCENARIOS = 'cargo test -p shell-simulator --locked --no-default-features --features cycling,debug-harness --test ride_scenarios'


@dataclass(frozen=True)
class Task:
    name: str
    help: str
    steps: tuple[str, ...] = ()
    needs: tuple[str, ...] = ()
    alias: str | None = None


def task(name, help, *steps, needs=(), alias=None):
    return Task(name, help, steps, tuple(needs), alias)


TASKS = [
    task('build', 'Build C606 firmware and its application image', 'bash devices/c606/build.sh'),
    task('test', 'Test host firmware logic and device tools', needs=['test-rust', 'test-trouble-host', 'test-python']),
    task(
        'test-rust',
        'Rust host tests (one target dir, so the passes run in turn); nextest runs every test binary of a pass at once',
        'cargo nextest run --workspace --locked --no-default-features',
        'cargo test --workspace --locked --no-default-features --doc',
        'cargo nextest run -p shell-simulator --locked --no-default-features --features debug-harness',
        'cargo test -p shell-simulator --locked --no-default-features --features debug-harness --doc',
        'cargo nextest run --workspace --locked --no-default-features --features shell-simulator/debug-harness,shell-simulator/cycling',
        'cargo test --workspace --locked --no-default-features --features shell-simulator/debug-harness,shell-simulator/cycling --doc',
    ),
    task(
        'test-trouble-host',
        'Vendored BLE host tests; their own workspace and target dir, so they run beside test-rust',
        'cargo test --manifest-path vendor/trouble-host/Cargo.toml --lib --locked',
    ),
    task(
        'test-python',
        'Device tool and terrain tests, spread over worker processes',
        'python -m pytest -q -n auto --maxprocesses=8 tools/devtools',
        f'{TERRAIN} pytest -q tools/terrain',
    ),
    task(
        'stack-frames',
        "Report a firmware ELF's largest stack frames against devices/c606/stack-frames.toml; --suggest prints new budgets",
        f'{DEV}stack_frames',
    ),
    task(
        'verify',
        'Run every PR gate in parallel (test, check, simulate, 4 builds, harness-virtual); --fast for the inner loop, --pr N to post results',
        f'{DEV}verify',
    ),
    task(
        'clean',
        'Free disk: remove merged, clean, idle worktrees and idle build dirs; dry run unless --apply, --idle-hours N (default 24)',
        f'{DEV}clean',
    ),
    task(
        'fmt',
        'Format Rust and Python code',
        'cargo fmt --all',
        'ruff format tools/devtools',
        f'{TERRAIN} ruff format tools/terrain',
    ),
    task(
        'check',
        'Check formatting and lint host code',
        'uv run --script tools/check-boundaries.py',
        'uv run --script apps/vana/web/build.py --check',
        'ruff format --check tools/devtools',
        'ruff check tools/devtools',
        f'{TERRAIN} ruff format --check tools/terrain',
        f'{TERRAIN} ruff check tools/terrain',
        'cargo check -p device-api --locked --features network-stack',
        'cargo clippy --workspace --all-targets --locked --no-default-features -- -D warnings',
        'cargo clippy --workspace --all-targets --locked --no-default-features --features shell-simulator/debug-harness,shell-simulator/cycling,shell-simulator/live -- -D warnings',
        'cargo fmt --all -- --check',
    ),
    task('terrain', 'Build, validate or trace offline terrain packs (docs/terrain.md)', f'{TERRAIN} cycling-terrain'),
    task(
        'web', 'Build the phone web client the device serves (apps/vana/web)', 'uv run --script apps/vana/web/build.py'
    ),
    task('backup', 'Read and verify a private full-flash backup from stock', f'{DEV}device.firmware backup'),
    task(
        'flash',
        'Flash cycling into the inactive A/B slot, boot it, and reselect the old slot if its console stays silent',
        f'{DEV}device.firmware flash',
        needs=['build'],
    ),
    task(
        'flash-image',
        'Flash the already built .local/cycling.bin like flash does, without building (device hosts receive images)',
        f'{DEV}device.firmware flash',
    ),
    task(
        'device',
        'Run a device task on CYCLING_DEVICE_HOST, the machine with the C606 (device flash builds here, flashes there)',
        f'{DEV}device.remote',
    ),
    task(
        'rollback',
        'Boot the previous cycling image in the other A/B slot after validating it',
        f'{DEV}device.firmware rollback',
    ),
    task(
        'stock',
        'Restore the complete verified 16 MiB stock backup (confirmation required; saves cycling rides/settings first)',
        f'{DEV}device.firmware stock',
    ),
    task(
        'restore-owned',
        'Write a saved ride/settings snapshot back under running cycling firmware, with verified readback',
        f'{DEV}device.firmware restore-owned',
    ),
    task(
        'stock-select',
        'Boot stock in slot A without rewriting flash, while slot A still holds the verified stock image',
        f'{DEV}device.firmware stock-select',
        alias='boot-stock',
    ),
    task('monitor', 'Read USB logs without resetting the device', f'{DEV}device.firmware monitor'),
    task('wifi-setup', 'Save current NetworkManager Wi-Fi credentials privately', f'{DEV}connectivity.wifi setup'),
    task(
        'ride-export',
        'Export the complete read-only ride prefix as private raw, JSON and GPX files',
        f'{DEV}rides.export',
    ),
    task(
        'ride-clear',
        'Clear the owned ride journal after verifying a named complete export',
        f'{DEV}rides.clear',
    ),
    task(
        'workout-load',
        "Put a workout JSON file on the device's FILES volume for the workout picker (--dry-run prints the file)",
        f'{DEV}workouts.load',
    ),
    task(
        'gps-stress',
        'Check GNSS progress and transport counters under repeated USB state replies',
        f'{DEV}harness.stress gps .local/tests/gps-stress-$(date +%s)',
    ),
    task(
        'companion-stress',
        'Check companion progress and transport counters under repeated USB state replies',
        f'{DEV}harness.stress companion .local/tests/companion-stress-$(date +%s)',
    ),
    task(
        'bluetooth-echo',
        'Verify C606 BLE echo notifications, reads and reconnect with this laptop',
        f'{SYSTEM_PYTHON}connectivity.bluetooth',
    ),
    task(
        'ble-simulator',
        'Advertise deterministic HRS and CSC fixtures from this laptop',
        f'{SYSTEM_PYTHON}connectivity.ble_sensor_sim',
    ),
    task(
        'logs',
        'Capture private JSON logs without resetting; send optional commands once',
        f'{DEV}terminal.logs',
    ),
    task(
        'backtrace',
        'Decode code addresses in device logs (files or stdin) against .local/cycling.elf',
        f'{DEV}terminal.backtrace',
    ),
    task('terminal', 'Run a command or open the interactive no-reset USB terminal', f'{DEV}terminal.cli'),
    task(
        'e2e',
        'Verify terminal framing, repeated no-reset opens and independent acquisition',
        f'{DEV}harness.base .local/tests/base-$(date +%s)',
    ),
    task(
        'persistence-test',
        'Save a temporary brightness, restart, verify and restore original preferences',
        f'{DEV}harness.base .local/tests/persistence-$(date +%s) --write-settings --restart',
    ),
    task(
        'regression',
        'Run a 60-second no-render terminal and acquisition regression',
        f'{DEV}harness.base .local/tests/regression-$(date +%s) --seconds 60',
    ),
    task(
        'crash-test',
        'Verify controlled panic, software reboot and one-shot marker through ordinary terminal',
        f'{DEV}harness.base .local/tests/crash-$(date +%s) --panic',
    ),
    task(
        'ant-export',
        'Decode historical ANT captures from a raw journal file into a private directory',
        f'{DEV}rides.ant_capture',
    ),
    task(
        'simulate',
        'Run the portable shell against deterministic display and input devices',
        'cargo run -p shell-simulator --locked --no-default-features --bin shell-simulator',
    ),
    task(
        'sim',
        'Run VANA scenarios in the simulated world; report and artifacts go to .local/sim/latest',
        'rm -rf .local/sim/latest && mkdir -p .local/sim/latest',
        f'SIM_ARTIFACTS=$PWD/.local/sim/latest {SIM_SCENARIOS} 2>&1 | tee .local/sim/latest/cargo.log || true',
        f'{DEV}sim.report .local/sim/latest',
    ),
    task(
        'sim-pr',
        'Run simulator scenarios and post their videos and screenshots inline on a pull request: sim-pr <pr>',
        'test -n "${1:-}" || { echo "usage: cycling sim-pr <pr>" >&2; exit 2; }',
        'rm -rf .local/sim/pr && mkdir -p .local/sim/pr',
        f'SIM_ARTIFACTS=$PWD/.local/sim/pr {SIM_SCENARIOS} 2>&1 | tee .local/sim/pr/cargo.log || true',
        f'{DEV}sim.report .local/sim/pr --pr "$1"',
    ),
    task(
        'sim-compare',
        'Run simulator scenarios here and on a base ref; report changed screens Before | After (--base REF, --pr N, --filter NAME)',
        f'{DEV}sim.compare',
    ),
    task(
        'sim-live',
        'Open a window onto the simulated device; the session is recorded to .local/sim/live',
        'SIM_ARTIFACTS=$PWD/.local/sim/live cargo run -p shell-simulator --locked --no-default-features --features cycling,debug-harness,live --bin sim-live',
    ),
    task(
        'harness',
        'Run a named or structured C606/virtual harness scenario and return compact JSON',
        f'{DEV}harness.runner',
    ),
    task(
        'screenshot',
        'Save the next frame pushed to the C606 panel as a PNG (harness builds); --scale 2, --press BUTTON',
        f'{DEV}harness.screenshot',
    ),
    task(
        'latency',
        'Time button press to finished frame on the C606 and the idle frame rate (harness builds); --press, --setup',
        f'{DEV}harness.latency',
    ),
    task(
        'startup-timing',
        'Measure power-on to first frame, Home, storage, GNSS, sensors, ride start and press latency (C606, or --sim)',
        f'{DEV}harness.startup_timing',
    ),
    task(
        'harness-virtual',
        'Run shared deterministic input, capture and restart scenarios',
        f'{DEV}harness.runner run shared --backend virtual',
        f'{DEV}harness.runner run virtual-foundation --backend virtual',
        f'{DEV}harness.runner run virtual-sdk --backend virtual --sdk',
    ),
    task(
        'mmc-inspect',
        'Read bounded MMC layout and root metadata into private local evidence',
        f'{DEV}device.mmc',
    ),
    task(
        'volumes',
        'Inspect the MMC volume table, terrain pack and file store (inspect|init|terrain-write|terrain-verify|files)',
        f'{DEV}device.volumes',
    ),
    task(
        'terrain-write',
        'Install a terrain pack file into the MMC TERRAIN volume, resumable, with --verify readback',
        f'{DEV}device.volumes terrain-write',
    ),
    task(
        'terrain-verify',
        'Compare the installed TERRAIN pack with a file, chunk by chunk on the device',
        f'{DEV}device.volumes terrain-verify',
    ),
    task(
        'files',
        'List, put, get, delete or format files in the MMC FILES store (list|put name path|get name path|delete name|format --confirm)',
        f'{DEV}device.volumes files',
    ),
    task(
        'mmc-maintenance',
        'Back up or explicitly write MMC using the dedicated maintenance image',
        f'{DEV}device.mmc_maintenance',
    ),
    task(
        'device-port',
        "Resolve the backed-up C606's current USB port without opening it",
        f'{DEV}device.port',
    ),
]
BY_NAME = {t.name: t for t in TASKS} | {t.alias: t for t in TASKS if t.alias}


def bash(step, task, args, last):
    """argv for one step; the last step gets the task's arguments unless it places them itself."""
    script = f'{step} "$@"' if last and '$1' not in step and '"$@"' not in step else step
    return ['bash', '-c', script, f'cycling {task.name}', *args]


def environment():
    """The steps' environment: no ambient PYTHONPATH shadows the locked uv environment, and
    VIRTUAL_ENV goes so nested `uv run --project` calls pick their own project."""
    return {k: v for k, v in os.environ.items() if k not in {'PYTHONPATH', 'VIRTUAL_ENV'}}


def run(task, args=(), env=None, replace=False):
    """Run needs, then steps in order. With replace, the last step takes over this process,
    so an interactive tool gets the terminal and its signals directly."""
    env = environment() if env is None else env
    if code := run_side_by_side(task.needs, env):
        return code
    for i, step in enumerate(task.steps):
        argv = bash(step, task, args, last=i == len(task.steps) - 1)
        if replace and i == len(task.steps) - 1:
            os.chdir(ROOT)
            os.execvpe('bash', argv, env)
        if code := subprocess.run(argv, cwd=ROOT, env=env).returncode:
            return code
    return 0


def run_side_by_side(names, env):
    """One need runs in the foreground; several run in parallel with prefixed output."""
    if len(names) == 1:
        return run(BY_NAME[names[0]], env=env)
    lock = threading.Lock()

    def prefixed(name):
        process = subprocess.Popen(
            [sys.executable, '-m', 'cycling_devtools.cli', name],
            cwd=ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors='replace',
        )
        for line in process.stdout:
            with lock:
                print(f'[{name}] {line}', end='', flush=True)
        return process.wait()

    with ThreadPoolExecutor(len(names) or 1) as pool:
        codes = list(pool.map(prefixed, names))
    return next((code for code in codes if code), 0)


def usage():
    width = max(len(t.name) for t in TASKS)
    rows = [f'  {t.name:<{width}}  {t.help}' + (f' (alias: {t.alias})' if t.alias else '') for t in TASKS]
    return 'usage: cycling <task> [args...]\n\ntasks:\n' + '\n'.join(rows)


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if not argv or argv[0] in {'-h', '--help', 'help'}:
        print(usage())
        return 0
    name, args = argv[0], argv[1:]
    # `mise run task -- args` habit: a leading -- separates nothing here.
    if args[:1] == ['--']:
        args = args[1:]
    if name not in BY_NAME:
        print(f'cycling: unknown task {name!r}\n\n{usage()}', file=sys.stderr)
        return 2
    return run(BY_NAME[name], args, replace=True)


if __name__ == '__main__':
    sys.exit(main())
