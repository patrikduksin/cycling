"""Run simulator scenarios on the merge-base and the current checkout, and compare them."""

import argparse
import fcntl
import functools
import hashlib
import os
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib
from concurrent.futures import Future, ThreadPoolExecutor
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools import runner
from cycling_devtools.clean import warn_if_low_disk
from cycling_devtools.sim import report
from cycling_devtools.workspace import ROOT, main_checkout

OUT = ROOT / '.local/sim/compare'
FILL = ROOT / '.local/sim/fill'
CACHE = main_checkout() / '.local/sim/screens-cache'
# A busy day of main commits (23 on 2026-11-14), at about 180 MB each.
CACHE_ENTRIES = 24
STAGING = '.staging-'
# Lock files are never deleted: a process waiting on one holds its inode, so a
# replaced file would let a second process build the same key.
LOCKS = 1024
ABANDONED_STAGING_S = 3600
COMMAND = [
    'cargo',
    'test',
    '-p',
    'shell-simulator',
    '--locked',
    '--no-default-features',
    '--test',
    'ride_scenarios',
]
# COMMAND builds it alongside the tests.
RENDERER = Path('debug/shell-simulator')
PNG = b'\x89PNG\r\n\x1a\n'
CHANNELS = {0: 1, 2: 3, 4: 2, 6: 4}
# Only these chunks are decoded; any other (PLTE, tRNS, gAMA, ...) falls back to bytes.
DECODED = {b'IHDR', b'IDAT', b'IEND'}
# How long cargo gets to exit on SIGTERM before its process group is killed.
GRACE_S = 5
TABLE = ['| Screen | Before | After |', '|---|---|---|']
DEFAULT_BASE = 'origin/main'
VIDEO_JOBS = 8


@dataclass(frozen=True)
class Pair:
    scenario: str
    checkpoint: str
    before: Path | None
    after: Path | None

    @functools.cached_property
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
    base_ref, base_commit, base_subject, head_commit = refs
    lines = [
        f'## {icon} Simulator before/after: {count["changed"]} changed · {count["new"]} new · '
        f'{count["removed"]} removed · {count["unchanged"]} unchanged',
        '',
        f'Before: the merge-base with `{base_ref}`, `{base_commit}` ({base_subject}), '
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
    previous = {r['name']: r for r in before}
    attachments = []
    for scenario in dict.fromkeys([*(p.scenario for p in pairs), *results]):
        shown = [(p, state) for p, state in states if p.scenario == scenario and state != 'unchanged']
        result = results.get(scenario)
        failed = result is not None and result['status'] != 'pass'
        if not shown and not failed:
            continue
        mark = '❌' if failed else '✅' if result else '⚪'
        suffix = ' (new scenario)' if scenario not in previous else ' (removed scenario)' if result is None else ''
        lines += ['', f'### {mark} {report.title(scenario)}{suffix}', '']
        if failed and result['failure']:
            lines += [*report.fenced(result['failure']), '']
        videos = [
            (label, r, video)
            for label, r in (('Before', previous.get(scenario)), ('After', result))
            if shown and r and (video := report.video(r))
        ]
        for label, r, video in videos:
            path = report.reference(r['dir'] / video, base)
            attachments.append(path)
            if len(videos) > 1:
                lines += [f'**{label}**', '']
            lines += [path if video.endswith('.mp4') else f'![video]({path})', '']
        if shown:
            lines += TABLE
        for p, state in shown:
            images = [None if i is None else report.reference(i, base) for i in (p.before, p.after)]
            attachments += [i for i in images if i]
            cells = ['—' if i is None else f'<img src="{i}" width="300">' for i in images]
            label = p.checkpoint if state == 'changed' else f'{p.checkpoint} · **{state}**'
            lines.append(f'| {label} | {cells[0]} | {cells[1]} |')
    lines += [
        '',
        '<sub>Generated by `cycling sim-compare`. Simulation does not cover DMA, display timing or radio coexistence.</sub>',
    ]
    return '\n'.join(lines) + '\n', attachments


def base_target(target):
    """The base's own cargo target directory, so neither build can serve the other.
    Sharing one let cargo reuse the base's test binary for the current checkout,
    because workspace artifacts are keyed by package, not by checkout path, and
    freshness by file times."""
    return target / 'sim-compare-base'


@dataclass(frozen=True)
class Run:
    source: Path
    artifacts: Path
    target: Path
    tests: tuple[str, ...] = ()


def selection(name_filter):
    return (name_filter,) if name_filter else ()


class Cargo:
    def __init__(self):
        self.lock = threading.Lock()
        self.running = set()
        self.stopping = False

    def start(self, command, **kwargs):
        with self.lock:
            if self.stopping:
                raise RuntimeError('sim-compare was interrupted')
            process = subprocess.Popen(command, **kwargs)
            self.running.add(process)
            return process

    def finish(self, process):
        stop(process)
        with self.lock:
            self.running.discard(process)

    def stop_all(self):
        with self.lock:
            self.stopping = True
            running = list(self.running)
        for process in running:
            stop(process)


CARGO = Cargo()


def run_scenarios(run):
    shutil.rmtree(run.artifacts, ignore_errors=True)
    run.artifacts.mkdir(parents=True)
    # Not CARGO_TARGET_DIR: sccache keys on every CARGO_* variable, so an
    # absolute path there would keep each worktree's dependency builds apart.
    env = {k: v for k, v in os.environ.items() if k not in ('CARGO_TARGET_DIR', 'SIM_VIDEO')}
    process = CARGO.start(
        [*COMMAND, '--target-dir', str(run.target), *run.tests],
        cwd=run.source,
        env=env | {'SIM_ARTIFACTS': str(run.artifacts)},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )
    try:
        with (run.artifacts / 'cargo.log').open('w') as log:
            for line in process.stdout:
                sys.stdout.write(f'{run.artifacts.name} | {line}')
                log.write(line)
        return process.wait()
    finally:
        CARGO.finish(process)


def stop(process):
    """End the whole process group, then reap its leader. Descendants can
    outlive or ignore SIGTERM, so the group always gets SIGKILL after a grace."""
    try:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=GRACE_S)
            except subprocess.TimeoutExpired:
                pass
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


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


def video_scenarios(pairs, before, after, with_changed):
    old, current = {r['name'] for r in before}, {r['name'] for r in after}
    new = [r['name'] for r in after if r['name'] not in old]
    if not with_changed:
        return new, []
    both = old & current
    differ = list(dict.fromkeys(p.scenario for p in pairs if p.scenario in both and p.state != 'unchanged'))
    return [*new, *differ], differ


def render_videos(renderer, results):
    def render(result):
        done = subprocess.run([renderer, '--video', result['dir']], capture_output=True, text=True)
        if done.returncode:
            print(f'sim-compare: warning: no video of {result["name"]}: {done.stderr.strip()}', file=sys.stderr)
        else:
            result['files'].append(Path(done.stdout.strip()).name)

    with ThreadPoolExecutor(VIDEO_JOBS) as pool:
        list(pool.map(render, results))


def rustc_hash():
    version = subprocess.run(['rustc', '-vV'], cwd=ROOT, check=True, capture_output=True, text=True).stdout
    return hashlib.sha256(version.encode()).hexdigest()[:12]


def cache_key(commit, rustc, name_filter):
    key = f'{commit}-{rustc}'
    if not name_filter:
        return key
    return f'{key}-{hashlib.sha256(name_filter.encode()).hexdigest()[:16]}'


@contextmanager
def flock(path, kind=fcntl.LOCK_EX):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a') as lock:
        fcntl.flock(lock, kind)
        yield


def cache_lock(kind):
    return flock(CACHE / '.lock', kind)


def build_lock(key):
    bucket = int(hashlib.sha256(key.encode()).hexdigest(), 16) % LOCKS
    return flock(CACHE / '.locks' / str(bucket))


def restore(key, directory):
    with cache_lock(fcntl.LOCK_SH):
        entry = CACHE / key
        if not entry.is_dir():
            return False
        shutil.rmtree(directory, ignore_errors=True)
        shutil.copytree(entry, directory)
        os.utime(entry)
    return True


def store(key, directory):
    CACHE.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=STAGING, dir=CACHE))
    try:
        shutil.copytree(directory, staging / key)
        with cache_lock(fcntl.LOCK_EX):
            # Fails when another run stored the same base first.
            with suppress(OSError):
                (staging / key).rename(CACHE / key)
                os.utime(CACHE / key)
            evict()
    finally:
        shutil.rmtree(staging, ignore_errors=True)


def evict():
    entries = [p for p in CACHE.iterdir() if p.is_dir() and not p.name.startswith('.')]
    entries.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    for entry in entries[CACHE_ENTRIES:]:
        shutil.rmtree(entry, ignore_errors=True)
    for staging in CACHE.glob(f'{STAGING}*'):
        with suppress(FileNotFoundError):
            if time.time() - staging.stat().st_mtime > ABANDONED_STAGING_S:
                shutil.rmtree(staging, ignore_errors=True)


def ensure_base(key, commit, out, target, tests):
    before = out / 'before'
    if restore(key, before):
        print(f'sim-compare: base screens from {CACHE / key}', flush=True)
        return None
    with build_lock(key):
        if restore(key, before):
            print(f'sim-compare: base screens from {CACHE / key}, built meanwhile', flush=True)
            return None
        return build(key, commit, out, target, tests)


def build(key, commit, out, target, tests):
    before, base = out / 'before', base_target(target)
    # Cargo releases its own lock while the tests run, so a second base build in
    # this target directory could replace the test binary before it starts.
    with flock(base.parent / f'{base.name}.lock'), checkout(commit, out / 'base-src') as source:
        status = run_scenarios(Run(source, before, base, tests))
    problem = incomplete(before, status)
    if problem is None:
        store(key, before)
    return problem


def fill(commit, name_filter=None):
    commit = git('rev-parse', '--verify', f'{commit}^{{commit}}')
    key = cache_key(commit, rustc_hash(), name_filter)
    with build_lock(key):
        if (CACHE / key).is_dir():
            return None
        FILL.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=f'{key}-', dir=FILL) as scratch:
            return build(key, commit, Path(scratch), cargo_target(), selection(name_filter))


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


def fetch_origin_main():
    """Bring origin/main up to date. Offline or without credentials, warn and keep the existing ref."""
    try:
        result = subprocess.run(
            # The explicit refspec updates origin/main even where the remote's configured refspec omits main.
            ['git', 'fetch', '--quiet', 'origin', '+refs/heads/main:refs/remotes/origin/main'],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=60,
            env={**os.environ, 'GIT_TERMINAL_PROMPT': '0'},
        )
        failure = None if result.returncode == 0 else result.stderr.strip() or f'exit status {result.returncode}'
    except (OSError, subprocess.TimeoutExpired) as error:
        failure = str(error)
    if failure:
        print(
            f'sim-compare: warning: could not fetch origin/main ({failure}); using the existing origin/main',
            file=sys.stderr,
        )


class NoBase(Exception):
    pass


def resolve_base(base):
    if base is None:
        fetch_origin_main()
        base = DEFAULT_BASE
    try:
        tip = git('rev-parse', '--verify', f'{base}^{{commit}}')
    except subprocess.CalledProcessError:
        raise NoBase(f'unknown git ref {base!r}') from None
    try:
        sha = git('merge-base', 'HEAD', tip)
    except subprocess.CalledProcessError as error:
        why = (error.stderr or '').strip() or 'they share no commit'
        raise NoBase(
            f'no merge-base of HEAD and {base} ({why}); a shallow clone or unrelated history has none'
        ) from None
    # log --format would print signature output first under log.showSignature.
    subject = git('log', '-1', '--no-show-signature', '--format=%s', sha)
    return base, sha, subject


def in_background(function, *args):
    """`function(*args)` in a daemon thread, so an interrupt never waits for it,
    even while it waits for another process's build lock."""
    future = Future()

    def run():
        try:
            future.set_result(function(*args))
        except BaseException as error:
            future.set_exception(error)

    threading.Thread(target=run, daemon=True).start()
    return future


def cargo_target():
    return Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target').resolve()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        '--base',
        help=f'compare against the merge-base with this git ref (default: {DEFAULT_BASE}, fetched from origin first)',
    )
    parser.add_argument('--pr', help='pull request to comment on with the comparison')
    parser.add_argument('--filter', help='cargo test name filter, applied to both runs')
    parser.add_argument(
        '--video', action='store_true', help='also show before and after videos of scenarios whose screens changed'
    )
    parser.add_argument(
        '--fill', metavar='COMMIT', help="only cache COMMIT's screens as a base for later comparisons, then exit"
    )
    parser.add_argument('--out', type=Path, default=OUT)
    args = parser.parse_args(argv)
    warn_if_low_disk()
    runner.unwind_on_termination()
    if args.fill:
        if problem := fill(args.fill, args.filter):
            print(f'sim-compare: {args.fill} was not cached: {problem}', file=sys.stderr)
            return 1
        return 0
    try:
        base, base_commit, base_subject = resolve_base(args.base)
    except NoBase as error:
        print(f'sim-compare: {error}', file=sys.stderr)
        return 2
    short = base_commit[:10]
    print(f'sim-compare base: merge-base with {base} at {short} {base_subject}', flush=True)
    # Absolute, because the base runs with its worktree as working directory.
    out = args.out.resolve()
    target = cargo_target()
    after_dir, before_dir = out / 'after', out / 'before'
    (out / 'summary.md').unlink(missing_ok=True)
    tests = selection(args.filter)
    key = cache_key(base_commit, rustc_hash(), args.filter)
    base_run = in_background(ensure_base, key, base_commit, out, target, tests)
    try:
        after_status = run_scenarios(Run(ROOT, after_dir, target, tests))
        before_problem = base_run.result()
    except BaseException:
        CARGO.stop_all()
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
        sys.stdout.flush()
        print('\nsim-compare: refusing to compare an incomplete run; nothing was reported or posted.', file=sys.stderr)
        for problem in problems:
            print(f'  {problem}', file=sys.stderr)
        return 1
    before, after = report.load(before_dir), report.load(after_dir)
    pairs = pair(before, after)
    after_videos, before_videos = video_scenarios(pairs, before, after, args.video)
    render_videos(
        target / RENDERER,
        [*(r for r in after if r['name'] in after_videos), *(r for r in before if r['name'] in before_videos)],
    )
    for directory, results in ((before_dir, before), (after_dir, after)):
        (directory / 'index.html').write_text(report.page(results, f'Simulator scenarios ({directory.name})'))
    markdown, attachments = comparison(pairs, before, after, ROOT, (base, short, base_subject, report.commit()))
    (out / 'summary.md').write_text(markdown)
    print(markdown.split('\n\n', 1)[0].removeprefix('## '))
    print(f'report {out / "summary.md"}; artifacts {before_dir}/index.html and {after_dir}/index.html')
    if args.pr:
        report.post(args.pr, markdown, attachments)
    return 1 if after_status or any(r['status'] != 'pass' for r in after) else 0


if __name__ == '__main__':
    raise SystemExit(main())
