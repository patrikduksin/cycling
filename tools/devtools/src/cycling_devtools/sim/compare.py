"""Run simulator scenarios on a base ref and the current checkout, and compare them.

The base runs in a temporary detached worktree under `.local/sim/compare`, built
into its own directory inside the current target directory (ignored with it). Sharing one directory let
cargo reuse the base's test binary for the current checkout afterwards, because
workspace artifacts are keyed by package, not by checkout path, and freshness by
file times. The separate directory stays incremental from run to run. Checkpoint
screenshots are paired by scenario and checkpoint name and compared by pixels,
so a reviewer sees Before | After only for screens that changed. Both sides run
without video; afterwards only the scenarios worth watching run again with it,
by exact test name: new ones, and with `--video` those whose screens changed.

A complete base run is kept in the primary checkout's `.local/sim/base-cache`,
keyed by base commit and filter, so later comparisons against the same commit
from any worktree skip the base checkout and build.
"""

import argparse
import fcntl
import functools
import hashlib
import os
import queue
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib
from contextlib import ExitStack, contextmanager, suppress
from dataclasses import dataclass
from pathlib import Path

from cycling_devtools.clean import warn_if_low_disk
from cycling_devtools.sim import report
from cycling_devtools.workspace import ROOT, main_checkout

OUT = ROOT / '.local/sim/compare'
CACHE = main_checkout() / '.local/sim/base-cache'
# About 70 MB each.
CACHE_ENTRIES = 4
STAGING = '.staging-'
# A staging directory this old belongs to a run that was killed.
STALE_S = 3600
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
PNG = b'\x89PNG\r\n\x1a\n'
CHANNELS = {0: 1, 2: 3, 4: 2, 6: 4}
# Only these chunks are decoded; any other (PLTE, tRNS, gAMA, ...) falls back to bytes.
DECODED = {b'IHDR', b'IDAT', b'IEND'}
# How long cargo gets to exit on SIGTERM before its process group is killed.
GRACE_S = 5
TABLE = ['| Screen | Before | After |', '|---|---|---|']
DEFAULT_BASE = 'origin/main'
TEST = re.compile(r'^test (\S+) \.\.\. ', re.MULTILINE)


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
        f'Before: `{base_ref}` at `{base_commit}` ({base_subject}), '
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
        section = 0
        videos = [
            (label, r, video)
            for label, r in (('Before', previous.get(scenario)), ('After', result))
            if shown and r and (video := report.video(r))
        ]
        for label, r, video in videos:
            path = report.reference(r['dir'] / video, base)
            attachments.append(path)
            section += 1
            if len(videos) > 1:
                lines += [f'**{label}**', '']
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
        '<sub>Generated by `cycling sim-compare`. Simulation does not cover DMA, display timing or radio coexistence.</sub>',
    ]
    return '\n'.join(lines) + '\n', attachments


def base_target(target):
    """The base's own cargo target directory, so neither build can serve the other."""
    return target / 'sim-compare-base'


@dataclass(frozen=True)
class Run:
    """The scenarios of the checkout at `source`, recorded into `artifacts`.
    `tests` are the cargo test arguments that select them."""

    source: Path
    artifacts: Path
    target: Path
    tests: tuple[str, ...] = ()
    video: bool = False


def selection(name_filter):
    return (name_filter,) if name_filter else ()


def exact(run_dir, names):
    """cargo test arguments that run only the tests that recorded `names` in
    `run_dir`. Some tests sit in modules, and a test that records more than one
    session names the others `<test>-<n>`."""
    log = (run_dir / 'cargo.log').read_text(errors='replace')
    paths = {path.rsplit('::', 1)[-1]: path for path in TEST.findall(log)}
    tests = (re.sub(r'-\d+$', '', name) for name in names)
    return ('--', '--exact', *dict.fromkeys(paths.get(test, test) for test in tests))


def scenarios(*runs):
    """Run each of `runs` into its artifacts at the same time, keeping cargo's
    log, and return cargo's exit statuses. cargo and its tests never outlive the
    call, so they cannot write into a worktree that is being removed."""
    processes, pumps, done = [], [], queue.Queue()
    try:
        for run in runs:
            shutil.rmtree(run.artifacts, ignore_errors=True)
            run.artifacts.mkdir(parents=True)
            env = {**os.environ, 'SIM_ARTIFACTS': str(run.artifacts), 'CARGO_TARGET_DIR': str(run.target)}
            env.pop('SIM_VIDEO', None)
            if run.video:
                env['SIM_VIDEO'] = '1'
            processes.append(
                subprocess.Popen(
                    [*COMMAND, *run.tests],
                    cwd=run.source,
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    start_new_session=True,
                )
            )
            prefix = f'{run.artifacts.name} | ' if len(runs) > 1 else ''
            pumps.append(threading.Thread(target=echo, args=(processes[-1], run.artifacts, prefix, done)))
            pumps[-1].start()
        for _ in pumps:
            if error := done.get():
                raise error
        return [process.wait() for process in processes]
    finally:
        for process in processes:
            stop(process)
        for pump in pumps:
            pump.join()


def echo(process, artifacts, prefix, done):
    """Copy cargo's output to its log and to stdout, then report how that went."""
    try:
        with (artifacts / 'cargo.log').open('w') as log:
            for line in process.stdout:
                sys.stdout.write(prefix + line)
                log.write(line)
        done.put(None)
    except BaseException as error:
        done.put(error)


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


def without_videos(results):
    """Drop videos a base recorder from before SIM_VIDEO wrote anyway, so only
    re-run scenarios show one."""
    for result in results:
        result['files'] = [f for f in result['files'] if not f.startswith('video.')]
    return results


def video_scenarios(pairs, before, after, changed):
    """(after, before) scenarios to re-run with video: new ones on the after
    side, and with `changed` those whose screens changed, on both sides."""
    old, current = {r['name'] for r in before}, {r['name'] for r in after}
    new = [r['name'] for r in after if r['name'] not in old]
    if not changed:
        return new, []
    both = old & current
    differ = list(dict.fromkeys(p.scenario for p in pairs if p.scenario in both and p.state != 'unchanged'))
    return [*new, *differ], differ


def record_videos(videos):
    """Run each (run, results, names) at once and move the videos it recorded
    for `names` beside their screens in `results`."""
    if not videos:
        return
    for (run, results, names), status in zip(videos, scenarios(*(run for run, _, _ in videos)), strict=True):
        if problem := incomplete(run.artifacts, status):
            print(f'sim-compare: warning: some videos are missing: {problem}', file=sys.stderr)
        by_name = {r['name']: r for r in results}
        for name in names:
            for video in sorted((run.artifacts / name).glob('video.*')):
                video.rename(by_name[name]['dir'] / video.name)
                by_name[name]['files'].append(video.name)


def cache_key(commit, name_filter):
    if not name_filter:
        return commit
    return f'{commit}-{hashlib.sha256(name_filter.encode()).hexdigest()[:16]}'


@contextmanager
def cache_lock(kind):
    """Readers share the lock; storing and evicting take it alone, so an entry
    is never removed while another sim-compare copies it."""
    CACHE.mkdir(parents=True, exist_ok=True)
    with (CACHE / '.lock').open('a') as lock:
        fcntl.flock(lock, kind)
        yield


def copy_run(source, destination):
    shutil.copytree(source, destination, ignore=shutil.ignore_patterns('video.*'))


def restore(key, directory):
    """Replace `directory` with the cached base run for `key`, if there is one."""
    with cache_lock(fcntl.LOCK_SH):
        entry = CACHE / key
        if not entry.is_dir():
            return False
        shutil.rmtree(directory, ignore_errors=True)
        copy_run(entry, directory)
        os.utime(entry)
    return True


def store(key, directory):
    """Cache a complete base run. It is copied aside and renamed into place, so
    an interrupted or concurrent run never leaves half an entry."""
    CACHE.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=STAGING, dir=CACHE))
    try:
        copy_run(directory, staging / key)
        with cache_lock(fcntl.LOCK_EX):
            # Fails when another run stored the same base first.
            with suppress(OSError):
                (staging / key).rename(CACHE / key)
                os.utime(CACHE / key)
            evict()
    finally:
        shutil.rmtree(staging, ignore_errors=True)


def evict():
    """Keep the most recently used entries; drop staging left by killed runs."""
    entries = [p for p in CACHE.iterdir() if p.is_dir() and not p.name.startswith('.')]
    entries.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    for entry in entries[CACHE_ENTRIES:]:
        shutil.rmtree(entry, ignore_errors=True)
    for staging in CACHE.glob(f'{STAGING}*'):
        # A live run removes its own staging without the lock.
        with suppress(FileNotFoundError):
            if time.time() - staging.stat().st_mtime > STALE_S:
                shutil.rmtree(staging, ignore_errors=True)


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


def resolve_base(base):
    """(ref, sha, subject) of the base. Without an explicit base, origin/main is fetched first."""
    if base is None:
        fetch_origin_main()
        base = DEFAULT_BASE
    # rev-parse resolves the commit itself; log --format would print signature output first under log.showSignature.
    sha = git('rev-parse', '--verify', f'{base}^{{commit}}')
    subject = git('log', '-1', '--no-show-signature', '--format=%s', sha)
    return base, sha, subject


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        '--base', help=f'git ref to compare against (default: {DEFAULT_BASE}, fetched from origin first)'
    )
    parser.add_argument('--pr', help='pull request to comment on with the comparison')
    parser.add_argument('--filter', help='cargo test name filter, applied to both runs')
    parser.add_argument(
        '--video', action='store_true', help='also show before and after videos of scenarios whose screens changed'
    )
    parser.add_argument('--out', type=Path, default=OUT)
    args = parser.parse_args(argv)
    warn_if_low_disk()
    try:
        base, base_commit, base_subject = resolve_base(args.base)
    except subprocess.CalledProcessError:
        print(f'unknown git ref {args.base or DEFAULT_BASE!r}', file=sys.stderr)
        return 2
    short = base_commit[:10]
    print(f'sim-compare base: {base} at {short} {base_subject}', flush=True)
    # Absolute, because the base runs with its worktree as working directory.
    out = args.out.resolve()
    target = Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target').resolve()
    after_dir, before_dir = out / 'after', out / 'before'
    (out / 'summary.md').unlink(missing_ok=True)
    tests = selection(args.filter)
    key = cache_key(base_commit, args.filter)
    with ExitStack() as stack:

        @functools.cache
        def base_source():
            return stack.enter_context(checkout(base_commit, out / 'base-src'))

        current = Run(ROOT, after_dir, target, tests)
        cached = restore(key, before_dir)
        if cached:
            print(f'sim-compare: base screens from {CACHE / key}', flush=True)
            [after_status], before_status = scenarios(current), 0
        else:
            after_status, before_status = scenarios(current, Run(base_source(), before_dir, base_target(target), tests))
        before_problem = incomplete(before_dir, before_status)
        if not cached and before_problem is None:
            store(key, before_dir)
        problems = [
            f'{side}: {problem}'
            for side, problem in (
                ('after (this checkout)', incomplete(after_dir, after_status)),
                (f'before ({base})', before_problem),
            )
            if problem
        ]
        if problems:
            sys.stdout.flush()
            print(
                '\nsim-compare: refusing to compare an incomplete run; nothing was reported or posted.', file=sys.stderr
            )
            for problem in problems:
                print(f'  {problem}', file=sys.stderr)
            return 1
        before, after = without_videos(report.load(before_dir)), without_videos(report.load(after_dir))
        pairs = pair(before, after)
        after_videos, before_videos = video_scenarios(pairs, before, after, args.video)
        videos = []
        if after_videos:
            run = Run(ROOT, out / 'after-video', target, exact(after_dir, after_videos), True)
            videos.append((run, after, after_videos))
        if before_videos:
            run = Run(base_source(), out / 'before-video', base_target(target), exact(before_dir, before_videos), True)
            videos.append((run, before, before_videos))
        record_videos(videos)
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
