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

from cycling_devtools.workspace import ROOT, SIMULATOR_BUILD

DEV = 'python -m cycling_devtools.'
# The terrain pack builder has its own environment (osmium, numpy, tifffile).
TERRAIN = 'uv run --project tools/terrain --locked'
# BlueZ tools need the distribution's D-Bus and GLib bindings, which only the
# system Python provides; they use the source tree instead of the uv environment.
SYSTEM_PYTHON = 'PYTHONPATH=tools/devtools/src /usr/bin/python3 -m cycling_devtools.'
SIM_SCENARIOS = 'cargo test -p shell-simulator --locked --no-default-features --test ride_scenarios'
TRACE = 'cargo run -q -p shell-simulator --locked --no-default-features --bin trace --'
FUZZ = 'cargo run -q -p shell-simulator --locked --no-default-features --bin fuzz --'
PRIMARY_CHECKOUT_RIDE_TRACES = (
    '"$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")/.local/ride-trace"'
)


@dataclass(frozen=True)
class Task:
    name: str
    help: str
    steps: tuple[str, ...] = ()
    needs: tuple[str, ...] = ()
    alias: str | None = None
    takes_device: bool = False


def task(name, help, *steps, needs=(), alias=None, takes_device=False):
    return Task(name, help, steps, tuple(needs), alias, takes_device)


TASKS = [
    task(
        'build',
        "Build a device's firmware and its application image; --variant NAME adds a [build].variants image",
        'bash devices/build.sh',
        takes_device=True,
    ),
    task(
        'build-maintenance',
        'Build the MMC maintenance image (binary USB protocol, no console) into .local/cycling.bin',
        'bash devices/build.sh --variant maintenance',
        takes_device=True,
    ),
    task(
        'devices',
        'Check every devices/*/device.toml and write the files generated from it; --locked fails on stale ones, '
        "--check compares the attached unit's FITTED reply with its manifest",
        f'{DEV}devices',
        takes_device=True,
    ),
    task(
        'test',
        'Test host firmware logic, device tools and the web packages',
        needs=['test-rust', 'test-trouble-host', 'test-python', 'test-web'],
    ),
    task(
        'test-rust',
        'Rust host tests: nextest runs every test binary at once, then the doc tests',
        'cargo nextest run --workspace --locked --no-default-features',
        'cargo test --workspace --locked --no-default-features --doc',
    ),
    task(
        'test-trouble-host',
        'Vendored BLE host tests; their own workspace and target dir, so they run beside test-rust',
        'cargo test --manifest-path vendor/trouble-host/Cargo.toml --lib --locked',
    ),
    task(
        'test-python',
        'Device tool and terrain tests, spread over worker processes',
        # With the simulator, so vana compiles once with the features both need.
        ' '.join((*SIMULATOR_BUILD, '-q', '-p', 'vana', '--bin', 'ride-fit')),
        'RIDE_FIT=${CARGO_TARGET_DIR:-target}/debug/ride-fit python -m pytest -q -n auto --maxprocesses=8 tools/devtools',
        f'{TERRAIN} pytest -q tools/terrain',
    ),
    task(
        'test-web',
        'Relay, site, bridge, protocol and flasher tests (web/; bun, files spread over worker processes)',
        'bun install --frozen-lockfile --silent',
        'bun test --parallel=8 web',
    ),
    task(
        'test-slow',
        'Every web test, one file at a time, including those too slow for every verify (CYCLING_SLOW=1)',
        'bun install --frozen-lockfile --silent',
        'CYCLING_SLOW=1 bun test web',
    ),
    task(
        'stack-frames',
        "Report a firmware ELF's largest stack frames against its device's stack-frames.toml; --suggest prints new budgets",
        f'{DEV}stack_frames',
        takes_device=True,
    ),
    task(
        'verify',
        'Run every PR gate in parallel (test, check, simulate, build, screens, harness-virtual); --fast for the inner loop, '
        '--pr N to post results and changed screens, --video for before/after videos of changed scenarios',
        f'{DEV}verify',
    ),
    task(
        'playbook-audit',
        "Print a session transcript's digest for the auditor in docs/agents/playbook-auditor.md, --session ID",
        f'{DEV}playbook_audit',
    ),
    task(
        'clean',
        'Free disk: remove merged, clean, idle worktrees and idle build dirs; dry run unless --apply, --idle-hours N (default 24)',
        f'{DEV}clean',
    ),
    task(
        'fmt',
        'Format Rust, Python and web code',
        'cargo fmt --all',
        'ruff format tools/devtools',
        f'{TERRAIN} ruff format tools/terrain',
        'bun install --frozen-lockfile --silent',
        'bun run format',
    ),
    task(
        'check',
        'Check formatting and lint host code',
        'uv run --script tools/check-boundaries.py',
        f'{DEV}devices --locked',
        'uv run --script apps/vana/web/build.py --check',
        'ruff format --check tools/devtools',
        'ruff check tools/devtools',
        f'{TERRAIN} ruff format --check tools/terrain',
        f'{TERRAIN} ruff check tools/terrain',
        'cargo check -p device-api --locked --features network-stack',
        'cargo clippy --workspace --all-targets --locked --no-default-features --features shell-simulator/live -- -D warnings',
        'cargo fmt --all -- --check',
        'bun install --frozen-lockfile --silent',
        'bun run typecheck',
        'bun run lint',
        'bun run format:check',
    ),
    task('terrain', 'Build, validate or trace offline terrain packs (docs/terrain.md)', f'{TERRAIN} cycling-terrain'),
    task(
        'web', 'Build the phone web client the device serves (apps/vana/web)', 'uv run --script apps/vana/web/build.py'
    ),
    task(
        'backup',
        'Read and verify a private full-flash backup from stock',
        f'{DEV}device.firmware backup',
        takes_device=True,
    ),
    task(
        'flash',
        'Flash cycling into the inactive A/B slot, boot it, and reselect the old slot if its console stays silent',
        f'{DEV}device.firmware flash',
        needs=['build'],
        takes_device=True,
    ),
    task(
        'flash-maintenance',
        'Build the MMC maintenance image and flash it into the inactive slot as flash does',
        f'{DEV}device.firmware flash',
        needs=['build-maintenance'],
        takes_device=True,
    ),
    task(
        'flash-image',
        'Flash the already built .local/cycling.bin like flash does, without building (device hosts receive images)',
        f'{DEV}device.firmware flash',
        takes_device=True,
    ),
    task(
        'device',
        'Run a device task on CYCLING_DEVICE_HOST, the machine with the C606 (device flash builds here, flashes there)',
        f'{DEV}device.remote',
        takes_device=True,
    ),
    task(
        'rollback',
        'Boot the previous cycling image in the other A/B slot after validating it',
        f'{DEV}device.firmware rollback',
        takes_device=True,
    ),
    task(
        'stock',
        'Restore the complete verified 16 MiB stock backup (confirmation required; cycling rides and settings are lost)',
        f'{DEV}device.firmware stock',
        takes_device=True,
    ),
    task(
        'stock-select',
        'Boot stock in slot A without rewriting flash, while slot A still holds the verified stock image',
        f'{DEV}device.firmware stock-select',
        alias='boot-stock',
        takes_device=True,
    ),
    task('monitor', 'Read USB logs without resetting the device', f'{DEV}device.firmware monitor', takes_device=True),
    task(
        'flasher',
        "Ask any ESP32 esptool detects, here or through a visitor's page: flasher info|read ADDR SIZE PATH|verify ADDR PATH|reboot",
        f'{DEV}device.flasher_cli',
    ),
    task(
        'remote',
        "Use a device plugged into a visitor's browser at cycling.computer as a local port: remote <code> [-- <cmd>]",
        f'{DEV}device.browser_bridge',
    ),
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
        'Run VANA scenarios in the simulated world; report and screenshots go to .local/sim/latest (SIM_VIDEO=1 adds videos)',
        'rm -rf .local/sim/latest && mkdir -p .local/sim/latest',
        f'SIM_ARTIFACTS=$PWD/.local/sim/latest {SIM_SCENARIOS} 2>&1 | tee .local/sim/latest/cargo.log || true',
        f'{DEV}sim.report .local/sim/latest',
    ),
    task(
        'sim-pr',
        'Run simulator scenarios and post their videos and screenshots inline on a pull request: sim-pr <pr>',
        'test -n "${1:-}" || { echo "usage: cycling sim-pr <pr>" >&2; exit 2; }',
        'rm -rf .local/sim/pr && mkdir -p .local/sim/pr',
        f'SIM_ARTIFACTS=$PWD/.local/sim/pr SIM_VIDEO=1 {SIM_SCENARIOS} 2>&1 | tee .local/sim/pr/cargo.log || true',
        f'{DEV}sim.report .local/sim/pr --pr "$1"',
    ),
    task(
        'fuzz',
        'Random rides, faults and button presses in the twin, each session reproducible from its seed; '
        'findings go to .local/fuzz ([--minutes N | --seed S | --seeds A-B | --fixed] [--jobs N])',
        f'{FUZZ} --corpus {PRIMARY_CHECKOUT_RIDE_TRACES} --out .local/fuzz',
    ),
    task(
        'fuzz-planted',
        'Prove the fuzzer: it finds the bug planted behind the planted-bug feature, and its seed finds it again',
        'CARGO_TARGET_DIR=target/planted cargo test -p shell-simulator --locked --no-default-features '
        '--features planted-bug --test fuzz_planted -- --nocapture',
    ),
    task(
        'redraw-probe',
        'Report how much of the screen each page changes from frame to frame under a sensor feed: redraw-probe [seconds] [case filter]',
        'cargo run -q -p shell-simulator --locked --no-default-features --bin redraw-probe --',
    ),
    task(
        'sim-compare',
        'Run simulator scenarios here and on a base ref; report changed screens Before | After '
        '(--base REF, --pr N, --filter NAME, --video for before/after videos of changed scenarios)',
        f'{DEV}sim.compare',
    ),
    task(
        'mockup-compare',
        'Check sim checkpoints named after the course navigation mockups (.local/sim/latest/*/NN-<screen>.png) '
        'against docs/design/course-navigation; sheets in .local/mockup ([SCREEN ...] [--checkpoint PNG|SVG])',
        f'{DEV}sim.mockup',
    ),
    task(
        'readme-screenshots',
        'Render the README device screenshots in docs/screenshots from simulator checkpoints',
        f'{DEV}sim.readme',
    ),
    task(
        'sim-live',
        'Open a window onto the simulated device; the session is recorded to .local/sim/live',
        'SIM_ARTIFACTS=$PWD/.local/sim/live SIM_VIDEO=1 cargo run -p shell-simulator --locked --no-default-features --features live --bin sim-live',
    ),
    task(
        'trace',
        'Read a ride trace as text: summary, timeline, find, otlp, replay, test-stub (--help; skill: ride-trace); '
        'pull [file] copies the newest or a named device trace into .local/traces/ and rebuilds the media it '
        'started on beside it (.origin)',
        f'if [ "${{1:-}}" = pull ]; then shift; {DEV}device.volumes trace-pull "$@"; else {TRACE} "$@"; fi',
    ),
    task(
        'replay',
        'Replay a window of a ride trace with screenshots, screen text and divergence: replay <trace> --from T --to T',
        f'{TRACE} replay',
    ),
    task(
        'harness',
        'Run a named or structured C606/virtual harness scenario and return compact JSON',
        f'{DEV}harness.runner',
    ),
    task(
        'screenshot',
        'Save the next frame pushed to the C606 panel as a PNG; --scale 2, --press BUTTON',
        f'{DEV}harness.screenshot',
    ),
    task(
        'latency',
        'Time button press to finished frame on the C606 and the idle frame rate; --press, --setup',
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
        f'{DEV}harness.runner run virtual-sdk --backend virtual',
    ),
    task(
        'mmc-inspect',
        'Read bounded MMC layout and root metadata into private local evidence',
        f'{DEV}device.mmc',
    ),
    task(
        'volumes',
        'Inspect the MMC volume table, terrain pack and file store (inspect|init|terrain-write|terrain-verify|files|store)',
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
        'format',
        "Report what each of VANA's stores holds; --confirm erases rides, settings and files and restarts the device",
        f'{DEV}device.format',
    ),
    task(
        'store',
        'List, get or delete files in the MMC STORE, such as ride traces, or erase it (list [dir/]|get name path|delete name|format --confirm)',
        f'{DEV}device.volumes store',
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


def take_device(args):
    rest, device = [], None
    items = iter(args)
    for arg in items:
        if arg == '--device':
            device = next(items, None)
            if not device:
                raise ValueError('--device needs a name from devices/<name>/device.toml')
        elif arg.startswith('--device='):
            device = arg.removeprefix('--device=')
        else:
            rest.append(arg)
    return rest, device


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
    task, env = BY_NAME[name], environment()
    if task.takes_device:
        try:
            args, device = take_device(args)
        except ValueError as error:
            print(f'cycling {name}: {error}', file=sys.stderr)
            return 2
        if device:
            env['CYCLING_DEVICE'] = device
    return run(task, args, env=env, replace=True)


if __name__ == '__main__':
    sys.exit(main())
