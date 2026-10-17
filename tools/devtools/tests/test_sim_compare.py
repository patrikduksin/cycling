import contextlib
import json
import os
import struct
import subprocess
import sys
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


IMAGE = bytes(range(48))
OTHER = bytes([*range(47), 0])


def scenario(run, name, screens, status='pass'):
    """A scenario whose checkpoint screens are {caption: png bytes}."""
    directory = run / name
    directory.mkdir(parents=True)
    files = ['video.mp4']
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
    scenario(after, 'ride', {'riding': png(OTHER, 4), 'new view': png(IMAGE, 4)})
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(compare.pair(old, new), old, new, tmp_path, ('main', 'aaa', 'bbb'))
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
    assert all(len(files) <= report.ATTACHMENT_LIMIT for _, files in report.comments(markdown, attachments))


def test_unchanged_run_says_so_and_failures_still_show(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    scenario(before, 'steady', {'home': png(IMAGE, 4)})
    scenario(after, 'steady', {'home': png(IMAGE, 4)}, status='fail')
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(compare.pair(old, new), old, new, tmp_path, ('main', 'aaa', 'bbb'))
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


def test_one_scenario_with_many_changes_splits_within_the_attachment_limit(tmp_path):
    before, after = tmp_path / 'before', tmp_path / 'after'
    many = 30
    scenario(before, 'busy', {f'screen {i}': png(IMAGE, 4) for i in range(many)})
    scenario(after, 'busy', {f'screen {i}': png(OTHER, 4) for i in range(many)})
    old, new = report.load(before), report.load(after)
    markdown, attachments = compare.comparison(compare.pair(old, new), old, new, tmp_path, ('main', 'aaa', 'bbb'))
    assert len(attachments) == 2 * many + 1
    assert '### ✅ Busy (continued)' in markdown
    chunks = report.comments(markdown, attachments)
    assert len(chunks) == 2
    assert all(len(files) <= report.ATTACHMENT_LIMIT for _, files in chunks)
    assert sorted(f for _, files in chunks for f in files) == sorted(attachments)
    # Every continued table has its own header row.
    assert all('| Screen | Before | After |' in body for body, _ in chunks)


def test_a_build_failure_or_empty_run_is_incomplete(tmp_path):
    (tmp_path / 'cargo.log').write_text('error[E0425]: cannot find value\n')
    assert 'did not finish (exit status 101)' in compare.incomplete(tmp_path, 101)
    (tmp_path / 'cargo.log').write_text('test result: ok. 0 passed; 0 failed\n')
    assert 'no scenario ran' in compare.incomplete(tmp_path, 0)
    scenario(tmp_path, 'ride', {'riding': png(IMAGE, 4)}, status='fail')
    (tmp_path / 'cargo.log').write_text('test ride ... FAILED\ntest result: FAILED. 0 passed; 1 failed\n')
    # Failing scenarios are still worth comparing.
    assert compare.incomplete(tmp_path, 101) is None


def test_base_and_current_never_share_a_target_directory(tmp_path, monkeypatch):
    targets = {}

    def fake(source, artifacts, target, name_filter):
        targets[artifacts.name] = target
        artifacts.mkdir(parents=True, exist_ok=True)
        scenario(artifacts, 'ride', {'riding': png(IMAGE, 4)})
        (artifacts / 'cargo.log').write_text('test ride ... ok\ntest result: ok. 1 passed\n')
        return 0

    @contextlib.contextmanager
    def checkout(commit, path):
        yield path

    monkeypatch.setattr(compare, 'scenarios', fake)
    monkeypatch.setattr(compare, 'checkout', checkout)
    monkeypatch.setattr(compare, 'git', lambda *args: 'abc1234')
    monkeypatch.setenv('CARGO_TARGET_DIR', str(tmp_path / 'target'))
    assert compare.main(['--out', str(tmp_path / 'out')]) == 0
    assert targets['after'] == tmp_path / 'target'
    assert targets['before'] == tmp_path / 'target-sim-compare-base'


def test_main_refuses_an_incomplete_baseline_before_reporting(tmp_path, monkeypatch):
    def fake(source, artifacts, target, name_filter):
        assert artifacts.is_absolute() and target.is_absolute()
        artifacts.mkdir(parents=True, exist_ok=True)
        if artifacts.name == 'after':
            scenario(artifacts, 'ride', {'riding': png(IMAGE, 4)})
            (artifacts / 'cargo.log').write_text('test ride ... ok\ntest result: ok. 1 passed\n')
            return 0
        (artifacts / 'cargo.log').write_text('error: could not compile\n')
        return 101

    @contextlib.contextmanager
    def checkout(commit, path):
        yield path

    posted = []
    monkeypatch.setattr(compare, 'scenarios', fake)
    monkeypatch.setattr(compare, 'checkout', checkout)
    monkeypatch.setattr(compare, 'git', lambda *args: 'abc1234')
    monkeypatch.setattr(report, 'post', lambda *args: posted.append(args))
    monkeypatch.chdir(tmp_path)
    assert compare.main(['--out', 'relative', '--pr', '1']) == 1
    assert posted == []
    assert not (tmp_path / 'relative/summary.md').exists()


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
        compare.scenarios(tmp_path, tmp_path / 'run', tmp_path / 'target', None)
    assert started[0].poll() is not None
    # The whole group is gone, grandchildren included, once init reaps them.
    deadline = time.monotonic() + 5
    with pytest.raises(ProcessLookupError):
        while time.monotonic() < deadline:
            os.killpg(started[0].pid, 0)
            time.sleep(0.05)
