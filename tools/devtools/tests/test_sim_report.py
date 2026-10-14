import json

from cycling_devtools.sim import report


def scenario(run, name, status='pass', checkpoints=0, failure=None):
    directory = run / name
    directory.mkdir(parents=True)
    files = ['video.mp4', *[f'{i + 1:02}-step-{i}.png' for i in range(checkpoints)], 'final.png', 'chart.png']
    for f in files:
        (directory / f).write_bytes(b'x')
    (directory / 'ride-1.gpx').write_text('<gpx/>')
    result = {
        'name': name,
        'status': status,
        'failure': failure,
        'simulated_ms': 125_000,
        'frames': 10,
        'files': files,
        'checkpoints': [],
        'rides': [{'id': 1, 'samples': 120, 'track_m': 2333, 'entries': ['Start', 'Finish']}],
    }
    (directory / 'result.json').write_text(json.dumps(result))


def test_summary_lists_failures_first_and_references_every_attachment(tmp_path):
    scenario(tmp_path, 'ride_records_track', checkpoints=2)
    scenario(tmp_path, 'dropout_recovers', status='fail', failure='expected "190"')
    results = report.load(tmp_path)
    markdown, attachments = report.summary(results, tmp_path, 'Simulator scenarios', 'abc123')
    assert markdown.startswith('## ❌ Simulator scenarios: 1/2 scenarios passed')
    assert markdown.index('Dropout recovers') < markdown.index('Ride records track')
    assert 'expected "190"' in markdown
    assert '2.33 km' in markdown
    for path in attachments:
        assert path.startswith('./')
        assert path in markdown
    # A failure always shows the final screen; a pass shows its checkpoints.
    assert './dropout_recovers/final.png' in attachments
    assert './ride_records_track/01-step-0.png' in attachments
    assert './ride_records_track/final.png' not in attachments


def test_checkpoints_are_capped_but_keep_the_last(tmp_path):
    scenario(tmp_path, 'long', checkpoints=7)
    video, checkpoints, chart = report.media(report.load(tmp_path)[0])
    assert video == 'video.mp4'
    assert chart == 'chart.png'
    assert len(checkpoints) == report.CHECKPOINT_LIMIT
    assert checkpoints[-1] == '07-step-6.png'


def test_comments_split_at_the_attachment_limit(tmp_path):
    for i in range(12):
        scenario(tmp_path, f'scenario_{i:02}', checkpoints=4)
    markdown, attachments = report.summary(report.load(tmp_path), tmp_path, 'Sim', 'abc')
    chunks = report.comments(markdown, attachments)
    assert len(chunks) > 1
    assert all(len(files) <= report.ATTACHMENT_LIMIT for _, files in chunks)
    assert sorted(f for _, files in chunks for f in files) == sorted(attachments)
    for body, files in chunks:
        assert all(f in body for f in files)


def test_page_links_every_artifact(tmp_path):
    scenario(tmp_path, 'ride', checkpoints=1)
    page = report.page(report.load(tmp_path), 'Sim')
    assert '<video controls src="ride/video.mp4">' in page
    assert 'ride/ride-1.gpx' in page


def test_uploaded_urls_replace_local_paths_in_order():
    files = ['./a/video.mp4', './a/01-x.png']
    body = 'video\n./a/video.mp4\n<img src="./a/01-x.png">'
    uploaded = (
        'Uploading\n\nhttps://github.com/user-attachments/assets/1111-aa\n\n'
        '![1](https://github.com/user-attachments/assets/2222-bb)'
    )
    resolved = report.resolve(body, files, uploaded)
    assert './a/' not in resolved
    assert 'https://github.com/user-attachments/assets/1111-aa' in resolved.splitlines()[1]
    assert '<img src="https://github.com/user-attachments/assets/2222-bb">' in resolved


def test_labelled_uploads_resolve_by_index_not_position():
    files = ['./a/video.mp4', './a/01-x.png', './a/02-y.png']
    body = './a/video.mp4\n![](./a/01-x.png) ![](./a/02-y.png)'
    uploaded = (
        '<img alt="2" src="https://github.com/user-attachments/assets/3333-cc">\n'
        'https://github.com/user-attachments/assets/1111-aa\n'
        '<img width="10" src="https://github.com/user-attachments/assets/2222-bb" alt="1">'
    )
    resolved = report.resolve(body, files, uploaded)
    assert resolved.splitlines()[0] == 'https://github.com/user-attachments/assets/1111-aa'
    assert (
        '![](https://github.com/user-attachments/assets/2222-bb) ![](https://github.com/user-attachments/assets/3333-cc)'
        in resolved
    )


def test_lost_uploads_fail_loudly():
    uploaded = '![0](https://github.com/user-attachments/assets/1111-aa)'
    try:
        report.resolve('x', ['./a/0.png', './a/1.png'], uploaded)
    except RuntimeError as error:
        assert '1 of 2' in str(error)
    else:
        raise AssertionError('resolve accepted a missing upload')


def test_cargo_log_marks_missing_and_aborted_scenarios_as_failed(tmp_path):
    scenario(tmp_path, 'wrote_artifacts')
    scenario(tmp_path, 'dropped_before_assert')
    (tmp_path / 'cargo.log').write_text(
        'running 4 tests\n'
        'test wrote_artifacts ... ok\n'
        'test dropped_before_assert ... FAILED\n'
        'test aborted_silently ... ok\n'
        'test known_gap ... ignored\n'
    )
    results = {r['name']: r for r in report.load(tmp_path)}
    assert results['wrote_artifacts']['status'] == 'pass'
    assert results['dropped_before_assert']['status'] == 'fail'
    assert results['aborted_silently']['status'] == 'fail'
    assert results['cargo_test']['failure'].startswith('cargo test did not finish')
    assert 'known_gap' not in results
    markdown, _ = report.summary(list(results.values()), tmp_path, 'Sim', 'abc')
    assert markdown.startswith('## ❌ Sim: 1/4 scenarios passed')
    report.page(list(results.values()), 'Sim')


def test_failures_cannot_break_their_fence_or_the_comment_split():
    fence = report.fenced('boom\n```\n### not a heading')
    assert fence[0] == '````text' and fence[-1] == '````'
    assert '\n### ' not in fence[1]
