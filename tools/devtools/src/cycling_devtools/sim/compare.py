"""Run simulator scenarios on a base ref and the current checkout, and compare them.

The base runs in a temporary detached worktree under `.local/sim/compare`, built
into the same cargo target directory so dependencies are reused. Checkpoint
screenshots are paired by scenario and checkpoint name and compared by pixels,
so a reviewer sees Before | After only for screens that changed.
"""

import argparse
import os
import shutil
import signal
import struct
import subprocess
import sys
import zlib
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools.sim import report
from cycling_devtools.workspace import ROOT

OUT = ROOT / '.local/sim/compare'
COMMAND = [
    'cargo',
    'test',
    '-p',
    'shell-simulator',
    '--locked',
    '--no-default-features',
    '--features',
    'cycling,debug-harness',
    '--test',
    'ride_scenarios',
]
PNG = b'\x89PNG\r\n\x1a\n'
CHANNELS = {0: 1, 2: 3, 4: 2, 6: 4}
# Only these chunks are decoded; any other (PLTE, tRNS, gAMA, ...) falls back to bytes.
DECODED = {b'IHDR', b'IDAT', b'IEND'}
TABLE = ['| Screen | Before | After |', '|---|---|---|']


@dataclass(frozen=True)
class Pair:
    scenario: str
    checkpoint: str
    before: Path | None
    after: Path | None

    @property
    def state(self):
        if self.before is None:
            return 'new'
        if self.after is None:
            return 'removed'
        return 'unchanged' if same(self.before, self.after) else 'changed'


def screens(result):
    """Checkpoint screenshots and the final screen by caption, in recorded order.
    A repeated caption gets a count so every screenshot keeps a partner."""
    keyed = {}
    for name in result['files']:
        if name.endswith('.png') and (name[:2].isdigit() or name == 'final.png'):
            label, count = 'final screen' if name == 'final.png' else report.caption(name), 1
            key = label
            while key in keyed:
                count += 1
                key = f'{label} ({count})'
            keyed[key] = result['dir'] / name
    return keyed


def pair(before, after):
    """Pairs in the after run's order; checkpoints only the base has come last."""
    old = {r['name']: screens(r) for r in before}
    new = {r['name']: screens(r) for r in after}
    pairs = []
    for scenario in [*new, *(s for s in old if s not in new)]:
        a, b = old.get(scenario, {}), new.get(scenario, {})
        for checkpoint in [*b, *(c for c in a if c not in b)]:
            pairs.append(Pair(scenario, checkpoint, a.get(checkpoint), b.get(checkpoint)))
    return pairs


def same(a, b):
    """Equal pixels; identical files skip decoding."""
    a, b = Path(a).read_bytes(), Path(b).read_bytes()
    return a == b or pixels(a) == pixels(b)


def pixels(data):
    """Header fields and unfiltered rows of a non-interlaced 8-bit grey or RGB
    PNG, so a different encoder or compression level alone is not a change.
    Anything this does not fully decode is returned as its bytes."""
    if not data.startswith(PNG):
        return data
    header, chunks, at = None, [], len(PNG)
    while at + 8 <= len(data):
        length, kind = struct.unpack('>I4s', data[at : at + 8])
        body = data[at + 8 : at + 8 + length]
        at += 12 + length
        if kind == b'IHDR':
            header = body
        elif kind == b'IDAT':
            chunks.append(body)
        elif kind == b'IEND':
            break
        elif kind not in DECODED:
            return data
    width, height, depth, color, _, _, interlace = struct.unpack('>IIBBBBB', header)
    if depth != 8 or interlace or color not in CHANNELS:
        return data
    raw = zlib.decompress(b''.join(chunks))
    bpp = CHANNELS[color]
    stride = width * bpp
    previous = bytearray(stride)
    rows = []
    for y in range(height):
        start = y * (stride + 1)
        kind, row = raw[start], bytearray(raw[start + 1 : start + 1 + stride])
        unfilter(kind, row, previous, bpp)
        rows.append(bytes(row))
        previous = row
    return header, b''.join(rows)


def unfilter(kind, row, up, bpp):
    if kind == 1:
        for i in range(bpp, len(row)):
            row[i] = (row[i] + row[i - bpp]) & 0xFF
    elif kind == 2:
        for i in range(len(row)):
            row[i] = (row[i] + up[i]) & 0xFF
    elif kind == 3:
        for i in range(len(row)):
            left = row[i - bpp] if i >= bpp else 0
            row[i] = (row[i] + (left + up[i]) // 2) & 0xFF
    elif kind == 4:
        for i in range(len(row)):
            a = row[i - bpp] if i >= bpp else 0
            b, c = up[i], up[i - bpp] if i >= bpp else 0
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            row[i] = (row[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 0xFF


def comparison(pairs, before, after, base, refs):
    """Markdown for one PR comment and the files it references."""
    states = [(p, p.state) for p in pairs]
    count = {s: sum(state == s for _, state in states) for s in ('changed', 'new', 'removed', 'unchanged')}
    passed = sum(r['status'] == 'pass' for r in after)
    icon = '✅' if passed == len(after) else '❌'
    base_ref, base_commit, head_commit = refs
    lines = [
        f'## {icon} Simulator before/after: {count["changed"]} changed · {count["new"]} new · '
        f'{count["removed"]} removed · {count["unchanged"]} unchanged',
        '',
        f'Before: `{base_ref}` at `{base_commit}`, '
        f'{sum(r["status"] == "pass" for r in before)}/{len(before)} scenarios passed. '
        f'After: `{head_commit}`, {passed}/{len(after)} passed. '
        'Screens are paired by scenario and checkpoint and compared by pixels.',
    ]
    unchanged = {}
    for p, state in states:
        if state == 'unchanged':
            unchanged.setdefault(p.scenario, []).append(p.checkpoint)
    if unchanged:
        lines += ['', f'<details><summary>{count["unchanged"]} unchanged screens</summary>', '']
        lines += [f'- {report.title(s)}: {", ".join(names)}' for s, names in unchanged.items()]
        lines += ['', '</details>']
    if count['unchanged'] == len(pairs):
        lines += ['', 'No screen changed.']

    results = {r['name']: r for r in after}
    old = {r['name'] for r in before}
    attachments = []
    for scenario in dict.fromkeys([*(p.scenario for p in pairs), *results]):
        shown = [(p, state) for p, state in states if p.scenario == scenario and state != 'unchanged']
        result = results.get(scenario)
        failed = result is not None and result['status'] != 'pass'
        if not shown and not failed:
            continue
        mark = '❌' if failed else '✅' if result else '⚪'
        suffix = ' (new scenario)' if scenario not in old else ' (removed scenario)' if result is None else ''
        lines += ['', f'### {mark} {report.title(scenario)}{suffix}', '']
        if failed and result['failure']:
            lines += [*report.fenced(result['failure']), '']
        video = result and next((f for f in result['files'] if f.startswith('video.')), None)
        section = 0
        if video and shown:
            path = report.reference(result['dir'] / video, base)
            attachments.append(path)
            section += 1
            lines += [path if video.endswith('.mp4') else f'![video]({path})', '']
        if shown:
            lines += TABLE
        for p, state in shown:
            images = [None if i is None else report.reference(i, base) for i in (p.before, p.after)]
            needed = sum(i is not None for i in images)
            # Comments split only between sections, so keep each within gh's limit.
            if section + needed > report.ATTACHMENT_LIMIT:
                lines += ['', f'### {mark} {report.title(scenario)} (continued)', '', *TABLE]
                section = 0
            section += needed
            attachments += [i for i in images if i]
            cells = ['—' if i is None else f'<img src="{i}" width="300">' for i in images]
            label = p.checkpoint if state == 'changed' else f'{p.checkpoint} · **{state}**'
            lines.append(f'| {label} | {cells[0]} | {cells[1]} |')
    lines += [
        '',
        '<sub>Generated by `mise run sim-compare`. Simulation does not cover DMA, display timing or radio coexistence.</sub>',
    ]
    return '\n'.join(lines) + '\n', attachments


def scenarios(source, artifacts, target, name_filter):
    """Run the scenarios of `source` into `artifacts`, keeping cargo's log, and
    return cargo's exit status. cargo and its tests never outlive the call, so
    they cannot write into a worktree that is being removed."""
    shutil.rmtree(artifacts, ignore_errors=True)
    artifacts.mkdir(parents=True)
    command = [*COMMAND, *([name_filter] if name_filter else [])]
    env = {**os.environ, 'SIM_ARTIFACTS': str(artifacts), 'CARGO_TARGET_DIR': str(target)}
    with (artifacts / 'cargo.log').open('w') as log:
        process = subprocess.Popen(
            command,
            cwd=source,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
        )
        try:
            for line in process.stdout:
                sys.stdout.write(line)
                log.write(line)
            return process.wait()
        finally:
            stop(process)


def stop(process):
    """Terminate and reap the process group of a process that is still running."""
    if process.poll() is not None:
        return
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            return
        try:
            process.wait(timeout=10)
            return
        except subprocess.TimeoutExpired:
            continue


def incomplete(directory, status):
    """Why a run cannot be compared, or None. Failing scenarios still compare;
    a build failure or a run without scenarios would show every screen as new
    or removed."""
    log = directory / 'cargo.log'
    if 'test result:' not in (log.read_text(errors='replace') if log.exists() else ''):
        return f'cargo test did not finish (exit status {status}); see {log}'
    if not any(directory.glob('*/result.json')):
        return 'no scenario ran; check --filter'
    return None


@contextmanager
def checkout(commit, path):
    """A detached worktree of `commit`, removed afterwards."""

    def remove():
        subprocess.run(['git', 'worktree', 'remove', '--force', str(path)], cwd=ROOT, capture_output=True)
        shutil.rmtree(path, ignore_errors=True)
        subprocess.run(['git', 'worktree', 'prune'], cwd=ROOT, capture_output=True)

    remove()
    path.parent.mkdir(parents=True, exist_ok=True)
    git('worktree', 'add', '--detach', str(path), commit)
    try:
        yield path
    finally:
        remove()


def git(*args):
    return subprocess.run(['git', *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--base', default='main', help='git ref to compare against (default: main)')
    parser.add_argument('--pr', help='pull request to comment on with the comparison')
    parser.add_argument('--filter', help='cargo test name filter, applied to both runs')
    parser.add_argument('--out', type=Path, default=OUT)
    args = parser.parse_args(argv)
    try:
        base_commit = git('rev-parse', '--short', '--verify', f'{args.base}^{{commit}}')
    except subprocess.CalledProcessError:
        print(f'unknown git ref {args.base!r}', file=sys.stderr)
        return 2
    # Absolute, because the base runs with its worktree as working directory.
    out = args.out.resolve()
    target = Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target').resolve()
    after_dir, before_dir = out / 'after', out / 'before'
    (out / 'summary.md').unlink(missing_ok=True)
    after_status = scenarios(ROOT, after_dir, target, args.filter)
    with checkout(base_commit, out / 'base-src') as source:
        before_status = scenarios(source, before_dir, target, args.filter)
    problems = [
        f'{side}: {problem}'
        for side, directory, status in (
            ('after (this checkout)', after_dir, after_status),
            (f'before ({args.base})', before_dir, before_status),
        )
        if (problem := incomplete(directory, status))
    ]
    if problems:
        sys.stdout.flush()
        print('\nsim-compare: refusing to compare an incomplete run; nothing was reported or posted.', file=sys.stderr)
        for problem in problems:
            print(f'  {problem}', file=sys.stderr)
        return 1
    before, after = report.load(before_dir), report.load(after_dir)
    for directory, results in ((before_dir, before), (after_dir, after)):
        (directory / 'index.html').write_text(report.page(results, f'Simulator scenarios ({directory.name})'))
    pairs = pair(before, after)
    markdown, attachments = comparison(pairs, before, after, ROOT, (args.base, base_commit, report.commit()))
    (out / 'summary.md').write_text(markdown)
    print(markdown.split('\n\n', 1)[0].removeprefix('## '))
    print(f'report {out / "summary.md"}; artifacts {before_dir}/index.html and {after_dir}/index.html')
    if args.pr:
        report.post(args.pr, markdown, attachments)
    return 1 if after_status or any(r['status'] != 'pass' for r in after) else 0


if __name__ == '__main__':
    raise SystemExit(main())
