import concurrent.futures
import contextlib
import json
import os
import shutil
import signal
import struct
import subprocess
import sys
import threading
import time
import zlib

import pytest

from cycling_devtools.sim import compare, report


def png(rgb, width, filter_type=0, level=6):
    """An RGB PNG whose rows all use one filter, so encodings differ but pixels match."""
    stride = width * 3
    rows, previous = [], bytes(stride)
    for y in range(len(rgb) // stride):
        row = rgb[y * stride : (y + 1) * stride]
        if filter_type == 1:
            encoded = bytes((row[i] - (row[i - 3] if i >= 3 else 0)) & 0xFF for i in range(stride))
        elif filter_type == 2:
            encoded = bytes((row[i] - previous[i]) & 0xFF for i in range(stride))
        else:
            encoded = row
        rows.append(bytes([filter_type]) + encoded)
        previous = row

    def chunk(kind, body):
        return struct.pack('>I', len(body)) + kind + body + struct.pack('>I', zlib.crc32(kind + body))

    header = struct.pack('>IIBBBBB', width, len(rgb) // stride, 8, 2, 0, 0, 0)
    return (
        compare.PNG
        + chunk(b'IHDR', header)
        + chunk(b'IDAT', zlib.compress(b''.join(rows), level))
        + chunk(b'IEND', b'')
    )


@pytest.fixture(autouse=True)
def cache(tmp_path, monkeypatch):
    monkeypatch.setattr(compare, 'CACHE', tmp_path / 'cache')
    return tmp_path / 'cache'


IMAGE = bytes(range(48))
OTHER = bytes([*range(47), 0])


def scenario(run, name, screens, status='pass', video=False):
    """A scenario whose checkpoint screens are {caption: png bytes}."""
    directory = run / name
    directory.mkdir(parents=True)
    files = []
    if video:
        files.append('video.mp4')
        (directory / 'video.mp4').write_bytes(b'v')
    for i, (caption, data) in enumerate(screens.items()):
        file = 'final.png' if caption == 'final screen' else f'{i + 1:02}-{caption.replace(" ", "-")}.png'
        (directory / file).write_bytes(data)
        files.append(file)
    result = {
        'name': name,
        'status': status,
        'failure': 'boom' if status != 'pass' else None,
        'simulated_ms': 1000,
        'frames': 1,
        'files': files,
        'rides': [],
    }
    (directory / 'result.json').write_text(json.dumps(result))


def test_equal_pixels_match_across_filters_and_compression():
    plain = png(IMAGE, 4)
    assert compare.pixels(png(IMAGE, 4, filter_type=1, level=1)) == compare.pixels(plain)
    assert compare.pixels(png(IMAGE, 4, filter_type=2, level=9)) == compare.pixels(plain)
    assert compare.pixels(png(OTHER, 4)) != compare.pixels(plain)
    assert compare.pixels(png(IMAGE, 2)) != compare.pixels(plain)


def test_pairs_by_scenario_and_checkpoint_name(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    scenario(before, 'ride', {'riding': png(IMAGE, 4), 'old view': png(IMAGE, 4), 'final screen': png(IMAGE, 4)})
    scenario(before, 'gone', {'final screen': png(IMAGE, 4)})
    # Reordered and re-encoded but equal, one changed, one new.
    scenario(
        after,
        'ride',
        {'new view': png(IMAGE, 4), 'riding': png(IMAGE, 4, filter_type=2), 'final screen': png(OTHER, 4)},
    )
    scenario(after, 'fresh', {'final screen': png(IMAGE, 4)})
    pairs = compare.pair(report.load(before), report.load(after))
    states = {(p.scenario, p.checkpoint): p.state for p in pairs}
    assert states == {
        ('ride', 'new view'): 'new',
        ('ride', 'riding'): 'unchanged',
        ('ride', 'final screen'): 'changed',
        ('ride', 'old view'): 'removed',
        ('fresh', 'final screen'): 'new',
        ('gone', 'final screen'): 'removed',
    }
    # The after run's order, with checkpoints only the base has last.
    assert [p.checkpoint for p in pairs if p.scenario == 'ride'] == ['new view', 'riding', 'final screen', 'old view']


def test_repeated_captions_keep_their_partners(tmp_path):
    scenario(tmp_path, 'loop', {'lap': png(IMAGE, 4)})
    result = report.load(tmp_path)[0]
    result['files'].append('02-lap.png')
    (tmp_path / 'loop/02-lap.png').write_bytes(png(OTHER, 4))
    assert list(compare.screens(result)) == ['lap', 'lap (2)']


def test_comparison_shows_only_differences_side_by_side(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    scenario(before, 'steady', {'home': png(IMAGE, 4)})
    scenario(before, 'ride', {'riding': png(IMAGE, 4), 'old view': png(IMAGE, 4)})
    scenario(after, 'steady', {'home': png(IMAGE, 4)})
    scenario(after, 'ride', {'riding': png(OTHER, 4), 'new view': png(IMAGE, 4)}, video=True)
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(
        compare.pair(old, new), old, new, tmp_path, ('origin/main', 'aaa', 'Base subject', 'bbb')
    )
    assert 'Before: the merge-base with `origin/main`, `aaa` (Base subject)' in markdown
    assert markdown.startswith('## ✅ Simulator before/after: 1 changed · 1 new · 1 removed · 1 unchanged')
    assert '- Steady: home' in markdown
    assert '### ✅ Steady' not in markdown
    assert '| Screen | Before | After |' in markdown
    assert (
        '| riding | <img src="./before/ride/01-riding.png" width="300"> | <img src="./after/ride/01-riding.png"'
        in markdown
    )
    assert '| new view · **new** | — | <img src="./after/ride/02-new-view.png"' in markdown
    assert '| old view · **removed** | <img src="./before/ride/02-old-view.png" width="300"> | — |' in markdown
    # The changed scenario's video stays inline; the unchanged one uploads nothing.
    assert './after/ride/video.mp4' in attachments
    assert not any('steady' in a for a in attachments)
    for path in attachments:
        assert path in markdown
    assert report.comments(markdown, attachments) == [(markdown, attachments)]


def test_videos_on_both_sides_show_as_before_and_after(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    scenario(before, 'ride', {'riding': png(IMAGE, 4)}, video=True)
    scenario(after, 'ride', {'riding': png(OTHER, 4)}, video=True)
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(
        compare.pair(old, new), old, new, tmp_path, ('origin/main', 'aaa', 'Base subject', 'bbb')
    )
    assert '**Before**\n\n./before/ride/video.mp4\n\n**After**\n\n./after/ride/video.mp4' in markdown
    assert attachments[:2] == ['./before/ride/video.mp4', './after/ride/video.mp4']


def test_unchanged_run_says_so_and_failures_still_show(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    scenario(before, 'steady', {'home': png(IMAGE, 4)})
    scenario(after, 'steady', {'home': png(IMAGE, 4)}, status='fail')
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(
        compare.pair(old, new), old, new, tmp_path, ('origin/main', 'aaa', 'Base subject', 'bbb')
    )
    assert markdown.startswith('## ❌ Simulator before/after: 0 changed · 0 new · 0 removed · 1 unchanged')
    assert 'No screen changed.' in markdown
    assert '### ❌ Steady' in markdown and 'boom' in markdown
    assert attachments == []


def with_chunk(data, kind, body):
    """`data` with an extra chunk before IEND; the comparison does not check CRCs."""
    end = data.rindex(b'IEND') - 4
    return data[:end] + struct.pack('>I', len(body)) + kind + body + b'\0\0\0\0' + data[end:]


def test_chunks_it_cannot_decode_compare_as_bytes():
    plain = png(IMAGE, 4)
    transparent = with_chunk(plain, b'tRNS', b'\0\0\0\0\0\0')
    assert compare.pixels(transparent) == transparent
    assert compare.pixels(with_chunk(plain, b'tRNS', b'\0\0\0\0\0\1')) != compare.pixels(transparent)
    palette = plain.replace(struct.pack('>BB', 8, 2), struct.pack('>BB', 8, 3), 1)
    assert compare.pixels(palette) == palette


def test_a_comparison_too_long_for_one_comment_splits_between_scenarios(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    for i in range(30):
        scenario(before, f'busy_{i:02}', {f'screen {j}': png(IMAGE, 4) for j in range(10)})
        scenario(after, f'busy_{i:02}', {f'screen {j}': png(OTHER, 4) for j in range(10)})
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(
        compare.pair(old, new), old, new, tmp_path, ('origin/main', 'aaa', 'Base subject', 'bbb')
    )
    chunks = report.comments(markdown, attachments)
    assert len(chunks) == 2
    assert sorted(f for _, files in chunks for f in files) == sorted(attachments)
    for body, files in chunks:
        uploaded = body
        for path in files:
            uploaded = uploaded.replace(path, 'https://github.com/user-attachments/assets/' + 'f' * 36)
        assert len(uploaded) <= report.BODY_LIMIT
        assert '| Screen | Before | After |' in body


def test_a_scenario_too_long_for_one_comment_splits_between_its_rows(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    screens = 500
    scenario(before, 'endless', {f'screen {j}': png(IMAGE, 4) for j in range(screens)})
    scenario(after, 'endless', {f'screen {j}': png(OTHER, 4) for j in range(screens)}, status='fail')
    (after / 'endless/result.json').write_text(
        (after / 'endless/result.json').read_text().replace('"boom"', json.dumps('boom ' * 50_000))
    )
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(
        compare.pair(old, new), old, new, tmp_path, ('origin/main', 'aaa', 'Base subject', 'bbb')
    )
    chunks = report.comments(markdown, attachments)
    assert len(chunks) > 1
    assert sorted(f for _, files in chunks for f in files) == sorted(attachments)
    rows = [line for body, _ in chunks for line in body.splitlines() if line.startswith('|') and '<img' in line]
    assert len(rows) == screens
    for body, files in chunks:
        uploaded = body
        for path in files:
            uploaded = uploaded.replace(path, 'https://github.com/user-attachments/assets/' + 'f' * 36)
        assert len(uploaded) <= report.BODY_LIMIT - report.NOTE_ROOM
        if files:
            assert '| Screen | Before | After |' in body
    headings = [line for body, _ in chunks for line in body.splitlines() if line.startswith('### ')]
    assert headings == ['### ❌ Endless', *['### ❌ Endless (continued)'] * (len(headings) - 1)]
    assert markdown.count('boom') < 1_000, 'the failure is cut, not repeated whole'


def test_a_build_failure_or_empty_run_is_incomplete(tmp_path):
    (tmp_path / 'cargo.log').write_text('error[E0425]: cannot find value\n')
    assert 'did not finish (exit status 101)' in compare.incomplete(tmp_path, 101)
    (tmp_path / 'cargo.log').write_text('test result: ok. 0 passed; 0 failed\n')
    assert 'no scenario ran' in compare.incomplete(tmp_path, 0)
    scenario(tmp_path, 'ride', {'riding': png(IMAGE, 4)}, status='fail')
    (tmp_path / 'cargo.log').write_text('test ride ... FAILED\ntest result: FAILED. 0 passed; 1 failed\n')
    # Failing scenarios are still worth comparing.
    assert compare.incomplete(tmp_path, 101) is None


SCREENS = {
    'before': {'steady': png(IMAGE, 4), 'ride': png(IMAGE, 4)},
    'after': {'steady': png(IMAGE, 4), 'ride': png(OTHER, 4), 'fresh': png(IMAGE, 4)},
}


def simulate(runs, broken=()):
    def one(run):
        assert run.artifacts.is_absolute() and run.target.is_absolute()
        runs.append(run)
        side = 'before' if run.target.name == 'sim-compare-base' else 'after'
        names = [n for n in SCREENS[side] if not run.tests or run.tests[0] in n]
        shutil.rmtree(run.artifacts, ignore_errors=True)
        run.artifacts.mkdir(parents=True)
        if side in broken:
            (run.artifacts / 'cargo.log').write_text('error: could not compile\n')
            return 101
        for name in names:
            scenario(run.artifacts, name, {'final screen': SCREENS[side][name]})
            (run.artifacts / name / 'frames.bin').write_text(side)
        log = ''.join(f'test {n} ... ok\n' for n in names)
        (run.artifacts / 'cargo.log').write_text(f'{log}test result: ok. {len(names)} passed\n')
        return 0

    return one


RENDERER = """
import pathlib, sys
directory = pathlib.Path(sys.argv[2])
(directory / 'video.mp4').write_text((directory / 'frames.bin').read_text())
print(directory / 'video.mp4')
"""


def fake_main(tmp_path, monkeypatch, runs, checkouts, broken=()):
    @contextlib.contextmanager
    def checkout(commit, path):
        checkouts.append(commit)
        yield path

    def git(*args):
        if args[0] == 'rev-parse':
            return 'abc1234'
        return 'Base subject' if args[0] == 'log' else 'abc1234'

    renderer = tmp_path / 'target/debug/shell-simulator'
    renderer.parent.mkdir(parents=True)
    renderer.write_text(f'#!{sys.executable}\n{RENDERER}')
    renderer.chmod(0o755)
    monkeypatch.setattr(compare, 'run_scenarios', simulate(runs, broken))
    monkeypatch.setattr(compare, 'checkout', checkout)
    monkeypatch.setattr(compare, 'git', git)
    monkeypatch.setattr(compare, 'rustc_hash', lambda: 'rust1')
    monkeypatch.setattr(compare, 'FILL', tmp_path / 'fill')
    monkeypatch.setattr(compare, 'fetch_origin_main', lambda: None)
    monkeypatch.setenv('CARGO_TARGET_DIR', str(tmp_path / 'target'))
    return lambda *args: compare.main(['--out', str(tmp_path / 'out'), *args])


def test_base_and_current_never_share_a_target_directory(tmp_path, monkeypatch):
    runs = []
    main = fake_main(tmp_path, monkeypatch, runs, [])
    assert main() == 0
    targets = {run.artifacts.name: run.target for run in runs}
    assert targets == {'after': tmp_path / 'target', 'before': tmp_path / 'target/sim-compare-base'}


def test_main_refuses_an_incomplete_baseline_before_reporting(tmp_path, monkeypatch):
    posted = []
    main = fake_main(tmp_path, monkeypatch, [], [], broken=('before',))
    monkeypatch.setattr(report, 'post', lambda *args: posted.append(args))
    monkeypatch.chdir(tmp_path)
    assert compare.main(['--out', 'relative', '--pr', '1']) == 1
    assert main() == 1
    assert posted == []
    assert not (tmp_path / 'relative/summary.md').exists()
    assert not (compare.CACHE / 'abc1234-rust1').exists()


@pytest.mark.parametrize('video', [False, True])
def test_videos_come_from_the_runs_own_frames(tmp_path, monkeypatch, video):
    runs = []
    main = fake_main(tmp_path, monkeypatch, runs, [])
    out = tmp_path / 'out'
    assert main(*(['--video'] if video else [])) == 0
    assert sorted(r.artifacts.name for r in runs) == ['after', 'before']
    summary = (out / 'summary.md').read_text()
    assert summary.startswith('## ✅ Simulator before/after: 1 changed · 1 new · 0 removed · 1 unchanged')
    assert 'out/after/fresh/video.mp4' in summary
    assert ('out/after/ride/video.mp4' in summary) == video
    assert ('out/before/ride/video.mp4' in summary) == video
    assert 'steady/video.mp4' not in summary
    assert (out / 'after/fresh/video.mp4').read_text() == 'after'
    if video:
        assert (out / 'before/ride/video.mp4').read_text() == 'before'
    assert not list(compare.CACHE.glob('*/*/video.*'))


def test_a_complete_base_is_cached_by_commit_rustc_and_filter(tmp_path, monkeypatch, cache):
    runs, checkouts = [], []
    main = fake_main(tmp_path, monkeypatch, runs, checkouts)
    assert main() == 0
    first = (tmp_path / 'out/summary.md').read_text()
    assert (cache / 'abc1234-rust1/ride/final.png').exists()
    assert (cache / 'abc1234-rust1/ride/frames.bin').exists()
    runs.clear()
    assert main() == 0
    assert [r.artifacts.name for r in runs] == ['after']
    assert checkouts == ['abc1234']
    assert (tmp_path / 'out/summary.md').read_text() == first
    assert main('--filter', 'ride') == 0
    assert 'before' in [r.artifacts.name for r in runs]
    monkeypatch.setattr(compare, 'rustc_hash', lambda: 'rust2')
    assert main() == 0
    assert len([p for p in cache.iterdir() if not p.name.startswith('.')]) == 3


def test_the_base_is_the_merge_base(monkeypatch):
    calls = []
    monkeypatch.setattr(compare, 'fetch_origin_main', lambda: calls.append('fetch'))
    fake_git(monkeypatch, calls)
    assert compare.resolve_base(None) == ('origin/main', 'base5678', 'Base subject')
    assert calls == [
        'fetch',
        ('rev-parse', '--verify', 'origin/main^{commit}'),
        ('merge-base', 'HEAD', 'main1234'),
        ('log', '-1', '--no-show-signature', '--format=%s', 'base5678'),
    ]


def test_fill_caches_a_commit_once_for_later_comparisons(tmp_path, monkeypatch, cache):
    runs, checkouts = [], []
    main = fake_main(tmp_path, monkeypatch, runs, checkouts)
    assert main('--fill', 'abc1234') == 0
    assert main('--fill', 'abc1234') == 0
    assert checkouts == ['abc1234']
    assert [r.artifacts.name for r in runs] == ['before']
    runs.clear()
    assert main() == 0
    assert [r.artifacts.name for r in runs] == ['after']


def test_concurrent_misses_on_one_base_build_it_once(tmp_path, monkeypatch, cache):
    runs, checkouts = [], []
    fake_main(tmp_path, monkeypatch, runs, checkouts)
    real = compare.run_scenarios

    def slow(run):
        time.sleep(0.3)
        return real(run)

    monkeypatch.setattr(compare, 'run_scenarios', slow)
    target = tmp_path / 'target'
    outs = [tmp_path / f'out{i}' for i in range(3)]
    with concurrent.futures.ThreadPoolExecutor(3) as pool:
        problems = list(pool.map(lambda out: compare.ensure_base('key', 'abc1234', out, target, ()), outs))
    assert problems == [None] * 3
    assert checkouts == ['abc1234']
    for out in outs:
        assert (out / 'before/ride/final.png').exists()


def run_dir(path, marker):
    scenario(path, 'ride', {'riding': png(IMAGE, 4)})
    (path / 'cargo.log').write_text(f'{marker}\ntest ride ... ok\ntest result: ok. 1 passed\n')
    return path


def test_concurrent_stores_leave_one_whole_entry(tmp_path, cache):
    runs = [run_dir(tmp_path / f'run{i}', f'run {i}') for i in range(4)]
    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        list(pool.map(lambda run: compare.store('abc', run), runs))
    assert [p.name for p in cache.iterdir() if not p.name.startswith('.')] == ['abc']
    assert not list(cache.glob(f'{compare.STAGING}*'))
    restored = tmp_path / 'restored'
    assert compare.restore('abc', restored)
    assert compare.incomplete(restored, 0) is None
    assert sorted(p.name for p in (restored / 'ride').iterdir()) == ['01-riding.png', 'result.json']


def test_cache_keeps_the_most_recently_used_entries(tmp_path, monkeypatch, cache):
    monkeypatch.setattr(compare, 'CACHE_ENTRIES', 2)
    run = run_dir(tmp_path / 'run', 'run')
    compare.store('old', run)
    compare.store('used', run)
    os.utime(cache / 'old', (0, 0))
    os.utime(cache / 'used', (0, 0))
    assert compare.restore('used', tmp_path / 'restored')
    stale = cache / f'{compare.STAGING}killed'
    stale.mkdir()
    os.utime(stale, (0, 0))
    compare.store('new', run)
    assert sorted(p.name for p in cache.iterdir() if not p.name.startswith('.')) == ['new', 'used']
    assert not stale.exists()
    assert not compare.restore('old', tmp_path / 'restored')


def test_evicting_a_key_never_lets_a_second_build_of_it_start(tmp_path, monkeypatch, cache):
    monkeypatch.setattr(compare, 'CACHE_ENTRIES', 1)
    run = run_dir(tmp_path / 'run', 'run')
    compare.store('k', run)
    os.utime(cache / 'k', (0, 0))
    building, finish, events = threading.Event(), threading.Event(), []

    def first():
        with compare.build_lock('k'):
            building.set()
            finish.wait(5)
            events.append('first finished')

    def second():
        with compare.build_lock('k'):
            events.append('second started')

    builder = threading.Thread(target=first)
    builder.start()
    building.wait(5)
    compare.store('newer', run)
    assert not (cache / 'k').exists()
    waiter = threading.Thread(target=second)
    waiter.start()
    waiter.join(0.5)
    assert events == [], 'a second build started while the first held the lock'
    finish.set()
    builder.join()
    waiter.join()
    assert events == ['first finished', 'second started']


def fill_commits(tmp_path, monkeypatch):
    @contextlib.contextmanager
    def checkout(commit, path):
        path.mkdir(parents=True, exist_ok=True)
        (path / 'commit').write_text(commit)
        yield path

    def run(run):
        time.sleep(0.2)
        commit = (run.source / 'commit').read_text()
        run.artifacts.mkdir(parents=True, exist_ok=True)
        scenario(run.artifacts, 'ride', {'final screen': png(IMAGE, 4)})
        (run.artifacts / 'ride/frames.bin').write_text(commit)
        (run.artifacts / 'cargo.log').write_text('test ride ... ok\ntest result: ok. 1 passed\n')
        return 0

    monkeypatch.setattr(compare, 'checkout', checkout)
    monkeypatch.setattr(compare, 'run_scenarios', run)
    monkeypatch.setattr(compare, 'git', lambda *args: args[-1].removesuffix('^{commit}'))
    monkeypatch.setattr(compare, 'rustc_hash', lambda: 'rust1')
    monkeypatch.setattr(compare, 'FILL', tmp_path / 'fill')
    monkeypatch.setenv('CARGO_TARGET_DIR', str(tmp_path / 'target'))


def test_overlapping_fills_of_different_commits_each_cache_their_own_screens(tmp_path, monkeypatch, cache):
    fill_commits(tmp_path, monkeypatch)
    with concurrent.futures.ThreadPoolExecutor(2) as pool:
        assert list(pool.map(compare.fill, ['aaa', 'bbb'])) == [None, None]
    for commit in ('aaa', 'bbb'):
        assert (cache / f'{commit}-rust1/ride/frames.bin').read_text() == commit
    assert list((tmp_path / 'fill').iterdir()) == [], 'a fill left its checkout or screens behind'


def fake_git(monkeypatch, calls):

    def git(*args):
        calls.append(args)
        if args[0] == 'rev-parse':
            return 'main1234'
        if args[0] == 'merge-base':
            return 'base5678'
        assert args[0] == 'log' and '--no-show-signature' in args
        return 'Base subject'

    monkeypatch.setattr(compare, 'git', git)


def test_a_missing_merge_base_is_named_as_such(tmp_path, monkeypatch, capsys):
    def git(*args):
        if args[0] == 'merge-base':
            raise subprocess.CalledProcessError(1, ['git', *args], '', '')
        return 'main1234'

    monkeypatch.setattr(compare, 'git', git)
    monkeypatch.setattr(compare, 'fetch_origin_main', lambda: None)
    assert compare.main(['--out', str(tmp_path / 'out')]) == 2
    err = capsys.readouterr().err
    assert 'no merge-base of HEAD and origin/main' in err and 'shallow clone' in err
    assert 'unknown git ref' not in err


@pytest.mark.parametrize('stopped_by', [KeyboardInterrupt, signal.SIGTERM])
def test_an_interrupt_or_a_termination_stops_both_runs_at_once(tmp_path, monkeypatch, restored_signals, stopped_by):
    started = []
    real = subprocess.Popen

    def popen(*args, **kwargs):
        started.append(real(*args, **kwargs))
        return started[-1]

    class Interrupted:
        def write(self, text):
            if text.startswith('after |'):
                if stopped_by is KeyboardInterrupt:
                    raise KeyboardInterrupt
                os.kill(os.getpid(), stopped_by)

        def flush(self):
            pass

    @contextlib.contextmanager
    def checkout(commit, path):
        path.mkdir(parents=True, exist_ok=True)
        yield path

    script = (
        'import os, sys, time\n'
        'if os.environ["SIM_ARTIFACTS"].endswith("before"):\n'
        '    print("base running", flush=True); time.sleep(60)\n'
        'time.sleep(1); print("after running", flush=True); time.sleep(60)\n'
    )
    monkeypatch.setattr(compare, 'COMMAND', [sys.executable, '-c', script])
    monkeypatch.setattr(compare.subprocess, 'Popen', popen)
    monkeypatch.setattr(compare.sys, 'stdout', Interrupted())
    monkeypatch.setattr(compare, 'checkout', checkout)
    monkeypatch.setattr(compare, 'git', lambda *args: 'Base subject' if args[0] == 'log' else 'abc1234')
    monkeypatch.setattr(compare, 'rustc_hash', lambda: 'rust1')
    monkeypatch.setattr(compare, 'fetch_origin_main', lambda: None)
    monkeypatch.setattr(compare, 'CARGO', compare.Cargo())
    monkeypatch.setenv('CARGO_TARGET_DIR', str(tmp_path / 'target'))
    began = time.monotonic()
    with pytest.raises(KeyboardInterrupt if stopped_by is KeyboardInterrupt else SystemExit):
        compare.main(['--out', str(tmp_path / 'out')])
    assert time.monotonic() - began < compare.GRACE_S
    assert len(started) == 2
    for process in started:
        assert process.wait(timeout=5) is not None
    assert not (compare.CACHE / 'abc1234-rust1').exists()


def test_explicit_base_is_resolved_without_fetching(monkeypatch):
    calls = []
    monkeypatch.setattr(compare, 'fetch_origin_main', lambda: calls.append('fetch'))
    fake_git(monkeypatch, calls)
    assert compare.resolve_base('main') == ('main', 'base5678', 'Base subject')
    assert 'fetch' not in calls
    assert calls[0] == ('rev-parse', '--verify', 'main^{commit}')


def test_signed_base_under_show_signature_is_not_parsed_as_sha(monkeypatch):
    # With log.showSignature=true, a log without --no-show-signature prints gpg lines before the commit line.
    def git(*args):
        if args[0] in ('rev-parse', 'merge-base'):
            return 'abc1234'
        if '--no-show-signature' not in args:
            return 'gpg: Signature made Sat Oct 17\ngpg: Good signature\nabc1234 Base subject'
        return 'Base subject'

    monkeypatch.setattr(compare, 'git', git)
    monkeypatch.setattr(compare, 'fetch_origin_main', lambda: None)
    assert compare.resolve_base(None) == ('origin/main', 'abc1234', 'Base subject')


def test_failed_fetch_warns_and_keeps_existing_origin_main(monkeypatch, capsys):
    seen = {}

    def run(args, **kwargs):
        seen['args'], seen['env'] = args, kwargs['env']
        return subprocess.CompletedProcess(args, 128, '', 'fatal: could not resolve host: github.com\n')

    monkeypatch.setattr(compare.subprocess, 'run', run)
    compare.fetch_origin_main()  # must not raise
    # The explicit refspec updates origin/main even where the remote's configured refspec omits main.
    assert seen['args'] == ['git', 'fetch', '--quiet', 'origin', '+refs/heads/main:refs/remotes/origin/main']
    assert seen['env']['GIT_TERMINAL_PROMPT'] == '0'
    err = capsys.readouterr().err
    assert 'could not fetch origin/main' in err and 'could not resolve host' in err and 'existing origin/main' in err


def test_a_run_logs_under_its_side_without_video_or_the_target_variable(tmp_path, monkeypatch, capsys):
    script = (
        'import os, sys; print(os.environ.get("SIM_VIDEO"), os.environ["SIM_ARTIFACTS"]);'
        'print(sys.argv[1:3] == ["--target-dir", os.environ.get("TARGET_SEEN")] and "CARGO_TARGET_DIR" not in os.environ);'
        'sys.exit(int(sys.argv[-1]))'
    )
    monkeypatch.setattr(compare, 'COMMAND', [sys.executable, '-c', script])
    monkeypatch.setenv('SIM_VIDEO', '1')
    monkeypatch.setenv('CARGO_TARGET_DIR', '/elsewhere')
    monkeypatch.setenv('TARGET_SEEN', str(tmp_path / 'target'))
    assert compare.run_scenarios(compare.Run(tmp_path, tmp_path / 'after', tmp_path / 'target', ('3',))) == 3
    log = (tmp_path / 'after/cargo.log').read_text().split()
    assert log[:2] == ['None', str(tmp_path / 'after')]
    # The target goes on the command line: sccache keys on every CARGO_* variable.
    assert log[2] == 'True'
    assert f'after | None {tmp_path / "after"}' in capsys.readouterr().out


def test_streaming_errors_stop_cargo_and_its_children(tmp_path, monkeypatch):
    started = []
    real = subprocess.Popen

    def popen(*args, **kwargs):
        started.append(real(*args, **kwargs))
        return started[-1]

    class Broken:
        def write(self, text):
            raise BrokenPipeError

    # The leader dies on SIGTERM; its child ignores SIGTERM and announces it.
    child = (
        'import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); print("ready", flush=True); time.sleep(60)'
    )
    script = f'import subprocess, sys, time; subprocess.Popen([sys.executable, "-c", {child!r}]); time.sleep(60)'
    monkeypatch.setattr(compare, 'COMMAND', [sys.executable, '-c', script])
    monkeypatch.setattr(compare.subprocess, 'Popen', popen)
    monkeypatch.setattr(compare.sys, 'stdout', Broken())
    with pytest.raises(BrokenPipeError):
        compare.run_scenarios(compare.Run(tmp_path, tmp_path / 'run', tmp_path / 'target'))
    assert started[0].poll() is not None
    # The whole group is gone, grandchildren included, once init reaps them.
    deadline = time.monotonic() + 5
    with pytest.raises(ProcessLookupError):
        while time.monotonic() < deadline:
            os.killpg(started[0].pid, 0)
            time.sleep(0.05)
