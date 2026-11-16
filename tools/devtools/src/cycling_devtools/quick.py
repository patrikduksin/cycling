import sys
import time
from concurrent.futures import ThreadPoolExecutor

from cycling_devtools import lanes, runner, tree, workspace
from cycling_devtools.lanes import Gate


def touched(paths, root):
    return {
        crate.directory: crate.name
        for crate in sorted(lanes.members(root), key=lambda crate: crate.directory)
        if any(path.startswith(f'{crate.directory}/') for path in paths)
    }


def gates(crates, firmware):
    if not crates:
        return []
    packages = ' '.join(f'-p {name}' for name in crates.values())
    found = [
        Gate(
            'test',
            f"cargo nextest run --locked --no-default-features --no-tests=pass {packages} -E 'not group(slow)'",
            (('NEXTEST_PROFILE', 'without-ride-scenarios'),),
        ),
        # Its own target dir, as in verify, so it does not queue on the tests' build lock.
        Gate(
            'clippy',
            f'cargo clippy --locked --no-default-features --all-targets {packages} -- -D warnings',
            (('CARGO_TARGET_DIR', 'target/clippy'),),
        ),
    ]
    if crates.keys() & firmware:
        found.append(Gate('check-firmware', 'cycling check-firmware'))
    return found


def run(running, gate):
    result = running.run(gate)
    print(f'{"ok  " if result.ok else "FAIL"} {gate.name} ({result.seconds:.1f} s)', flush=True)
    return result


def main():
    runner.stop_on_signals()
    root = workspace.ROOT
    paths = tree.changed_paths(tree.merge_base('HEAD'), tree.tree_hash(root))
    crates = touched(paths, root)
    plan = gates(crates, lanes.firmware_crates(root))
    if not plan:
        print('cycling quick: the diff touches no Rust crate')
        return 0
    print(f'cycling quick: {", ".join(crates.values())}', flush=True)
    logs = root / '.local' / 'verify' / 'quick'
    logs.mkdir(parents=True, exist_ok=True)
    running = runner.Gates(root, logs)
    started, cpu = time.monotonic(), runner.children_cpu()
    with ThreadPoolExecutor(len(plan)) as pool:
        try:
            results = list(pool.map(lambda gate: run(running, gate), plan))
        except BaseException:
            running.stop()
            raise
    print(f'{time.monotonic() - started:.1f} s, {runner.children_cpu() - cpu:.0f} CPU-s')
    if failures := runner.failure_tail(results):
        print(failures, file=sys.stderr)
    return 0 if all(result.ok for result in results) else 1


if __name__ == '__main__':
    sys.exit(main())
