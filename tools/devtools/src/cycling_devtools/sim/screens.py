"""Render every screen on every panel here and on the merge-base with main, and sheet them before | after."""

import argparse
import hashlib
import json
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import NamedTuple

from cycling_devtools import runner
from cycling_devtools.clean import warn_if_low_disk
from cycling_devtools.sim import compare, report
from cycling_devtools.workspace import ROOT

OUT = ROOT / '.local/sim/screens'
WALK = Path('tools/simulator/tests/screens.rs')
COMMAND = (*compare.COMMAND[:-1], WALK.stem)
ARTIFACTS_ENV = 'SIM_SCREENS'
UNREACHABLE = 'unreachable.tsv'
ORDER = ('changed', 'new', 'removed', 'unchanged')
FAILED = re.compile(r'^test (\S+) \.\.\. FAILED$', re.M)


def cache_key(commit, rustc, walk):
    return f'screens-{commit}-{rustc}-{hashlib.sha256(walk).hexdigest()[:16]}'


def run_walk(source, cells, target):
    return compare.run_scenarios(compare.Run(source, cells, target, command=COMMAND, artifacts_env=ARTIFACTS_ENV))


def incomplete(directory, status):
    log = directory / 'cargo.log'
    if 'test result:' not in (log.read_text(errors='replace') if log.exists() else ''):
        return f'cargo test did not finish (exit status {status}); see {log}'
    return None


def failed(directory):
    return FAILED.findall((directory / 'cargo.log').read_text(errors='replace'))


def walk_this_checkout(source, walk):
    (source / WALK).write_bytes(walk)


def ensure_base(key, commit, out, target, walk):
    before = out / 'before'
    if compare.restore(key, before):
        print(f'screens: base cells from {compare.CACHE / key}', flush=True)
        return None
    with compare.build_lock(key):
        if compare.restore(key, before):
            print(f'screens: base cells from {compare.CACHE / key}, built meanwhile', flush=True)
            return None
        base = compare.base_target(target)
        with compare.flock(base.parent / f'{base.name}.lock'), compare.checkout(commit, out / 'base-src') as source:
            walk_this_checkout(source, walk)
            status = run_walk(source, before, base)
        problem = incomplete(before, status)
        if problem is None:
            compare.store(key, before)
        return problem


def cells(directory):
    return {str(p.relative_to(directory).with_suffix('')): p for p in sorted(directory.glob('*/*.png'))}


class Cell(NamedTuple):
    state: str
    before: Path | None
    after: Path | None


def state(before, after):
    if before is None:
        return 'new'
    if after is None:
        return 'removed'
    return 'unchanged' if compare.same(before, after) else 'changed'


def compared(before_dir, after_dir):
    old, new = cells(before_dir), cells(after_dir)
    screens = {}
    for name in sorted(old.keys() | new.keys()):
        screen, cell = name.split('/', 1)
        variant, panel = cell.rsplit('@', 1)
        before, after = old.get(name), new.get(name)
        screens.setdefault(screen, {}).setdefault(variant, {})[panel] = Cell(state(before, after), before, after)
    return screens


def by_area(panel):
    width, height = map(int, panel.split('x'))
    return width * height


def sheet(renderer, screen, variants, directory):
    panels = sorted({p for row in variants.values() for p in row}, key=by_area)
    rows = [
        {
            'label': variant,
            'cells': {
                panel: {
                    'state': cell.state,
                    'before': cell.before and str(cell.before),
                    'after': cell.after and str(cell.after),
                }
                for panel, cell in row.items()
            },
        }
        for variant, row in variants.items()
    ]
    spec = directory / f'{screen}.json'
    spec.write_text(json.dumps({'title': f'{screen} - left before, right after', 'panels': panels, 'rows': rows}))
    png = directory / f'{screen}.png'
    done = subprocess.run([renderer, '--sheet', spec, png], capture_output=True, text=True)
    if done.returncode:
        raise RuntimeError(f'no sheet of {screen}: {done.stderr.strip()}')
    return png


def index(screens, sheets, refs, failures, unreachable):
    base, short, subject, head = refs
    flat = [
        (f'{screen}/{variant}@{panel}', cell.state)
        for screen, variants in screens.items()
        for variant, row in variants.items()
        for panel, cell in row.items()
    ]
    count = {s: sum(state == s for _, state in flat) for s in ORDER}
    lines = [
        f'# Screens: {count["changed"]} changed of {len(flat)} cells',
        '',
        f'{count["new"]} new, {count["removed"]} removed, {count["unchanged"]} unchanged. '
        f'Before: the merge-base with `{base}`, `{short}` ({subject}). After: `{head}`. '
        "Both sides run this checkout's walk; cells are compared by pixels.",
    ]
    for side, names in failures.items():
        if names:
            lines += ['', f'Walk flows that failed on {side}, so their later cells are missing: {", ".join(names)}.']
    touched = [
        screen
        for screen, variants in screens.items()
        if any(cell.state != 'unchanged' for row in variants.values() for cell in row.values())
    ]
    for screen in touched:
        lines += ['', f'## {screen}', '']
        lines += [f'- `{name}` {s}' for name, s in flat if name.startswith(f'{screen}/') and s != 'unchanged']
        lines += ['', f'![{screen}](sheets/{sheets[screen].name})']
    rest = [screen for screen in screens if screen not in touched]
    if rest:
        lines += ['', '## Unchanged', '']
        lines += [f'- [{screen}](sheets/{sheets[screen].name})' for screen in rest]
    if unreachable:
        lines += ['', '## Not reached', '', 'The walk reaches no cell of these:', '']
        lines += [f'- `{cell}`: {why}' for cell, why in unreachable]
    return '\n'.join(lines) + '\n', count['changed'], len(flat)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        '--base',
        help=f'compare against the merge-base with this git ref (default: {compare.DEFAULT_BASE}, fetched first)',
    )
    parser.add_argument('--out', type=Path, default=OUT)
    args = parser.parse_args(argv)
    warn_if_low_disk()
    runner.unwind_on_termination()
    try:
        base, base_commit, base_subject = compare.resolve_base(args.base)
    except compare.NoBase as error:
        print(f'screens: {error}', file=sys.stderr)
        return 2
    short = base_commit[:10]
    print(f'screens base: merge-base with {base} at {short} {base_subject}', flush=True)
    out, target = args.out.resolve(), compare.cargo_target()
    after_dir, before_dir, sheets_dir = out / 'after', out / 'before', out / 'sheets'
    walk = (ROOT / WALK).read_bytes()
    key = cache_key(base_commit, compare.rustc_hash(), walk)
    base_run = compare.in_background(ensure_base, key, base_commit, out, target, walk)
    try:
        after_status = run_walk(ROOT, after_dir, target)
        before_problem = base_run.result()
    except BaseException:
        compare.CARGO.stop_all()
        raise
    problems = [
        f'{side}: {problem}'
        for side, problem in (
            ('after (this checkout)', incomplete(after_dir, after_status)),
            (f'before (merge-base with {base})', before_problem),
        )
        if problem
    ]
    if problems:
        print('\nscreens: a side did not finish, so nothing was compared.', file=sys.stderr)
        for problem in problems:
            print(f'  {problem}', file=sys.stderr)
        return 1
    screens = compared(before_dir, after_dir)
    sheets_dir.mkdir(parents=True, exist_ok=True)
    for stale in sheets_dir.iterdir():
        stale.unlink()
    renderer = target / compare.RENDERER
    with ThreadPoolExecutor(compare.VIDEO_JOBS) as pool:
        made = pool.map(lambda item: sheet(renderer, *item, sheets_dir), screens.items())
        sheets = dict(zip(screens, made, strict=True))
    unreachable_file = after_dir / UNREACHABLE
    unreachable = (
        [line.split('\t', 1) for line in unreachable_file.read_text().splitlines()] if unreachable_file.exists() else []
    )
    failures = {'the base': failed(before_dir), 'this checkout': failed(after_dir)}
    markdown, changed, total = index(
        screens, sheets, (base, short, base_subject, report.commit()), failures, unreachable
    )
    (out / 'index.md').write_text(markdown)
    print(f'screens: {changed} changed of {total} cells; {out / "index.md"}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
