import json
import struct
import zlib

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
