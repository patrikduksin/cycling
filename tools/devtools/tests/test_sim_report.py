import json
import re
from pathlib import Path

import pytest

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


def test_comments_split_only_at_github_size_limit(tmp_path):
    for i in range(12):
        scenario(tmp_path, f'scenario_{i:03}', checkpoints=4)
    markdown, attachments = report.summary(report.load(tmp_path), tmp_path, 'Sim', 'abc')
    assert len(attachments) > report.ATTACHMENT_LIMIT
    assert report.comments(markdown, attachments) == [(markdown, attachments)]
    for i in range(12, 150):
        scenario(tmp_path, f'scenario_{i:03}', checkpoints=4)
    markdown, attachments = report.summary(report.load(tmp_path), tmp_path, 'Sim', 'abc')
    chunks = report.comments(markdown, attachments)
    assert len(chunks) > 1
    assert sorted(f for _, files in chunks for f in files) == sorted(attachments)
    for body, files in chunks:
        assert all(f in body for f in files)
        uploaded = body
        for path in files:
            uploaded = uploaded.replace(path, 'https://github.com/user-attachments/assets/' + 'f' * 36)
        assert len(uploaded) <= report.BODY_LIMIT


class FakeGitHub:
    def __init__(self, fail_uploads=False, fail_page=None):
        self.comments, self.next, self.uploads, self.fail_uploads = {}, 1, [], fail_uploads
        self.fail_page, self.pages = fail_page, 0

    def __call__(self, command, stdin=None):
        if command[:3] == ['gh', 'pr', 'comment']:
            files = command[command.index('--body-file') + 3 :: 2]
            if files and self.fail_uploads:
                raise RuntimeError(' '.join(command) + ' upload refused')
            if not files:
                self.pages += 1
                if self.pages == self.fail_page:
                    raise RuntimeError('HTTP 502: posting the page failed')
            self.uploads.append(len(files))
            body = stdin
            for i, path in enumerate(files):
                url = f'https://github.com/user-attachments/assets/{self.next:04}-{i:04}'
                # gh shows videos as bare URLs and images as Markdown.
                body = re.sub(
                    rf'!\[(\d+)\]\({re.escape(path)}\)', url if path.endswith('.mp4') else rf'![\1]({url})', body
                )
            self.comments[self.next] = body
            self.next += 1
            return f'https://github.com/o/r/pull/7#issuecomment-{self.next - 1}\n'
        number = int(next(c for c in command if '/comments/' in c).rsplit('/', 1)[-1])
        if 'DELETE' in command:
            del self.comments[number]
        elif 'PATCH' in command:
            self.comments[number] = stdin
        else:
            return self.comments[number]
        return ''


def posted(tmp_path, monkeypatch, github, scenarios=20):
    root = tmp_path / 'root'
    for i in range(scenarios):
        scenario(root / 'run', f'scenario_{i:03}', checkpoints=4)
    markdown, attachments = report.summary(report.load(root / 'run'), root, 'Sim', 'abc')
    spawned = []
    monkeypatch.setattr(report, 'ROOT', root)
    monkeypatch.setattr(report, 'PUBLISH', root / 'publish')
    monkeypatch.setattr(report, 'gh', github)
    monkeypatch.setattr(report.subprocess, 'Popen', lambda command, **kwargs: spawned.append(command))
    report.post(7, markdown, attachments)
    [command] = spawned
    assert command[-2] == '--publish'
    return attachments, Path(command[-1])


def test_the_report_posts_at_once_and_its_media_fill_in_one_comment(tmp_path, monkeypatch):
    github = FakeGitHub()
    attachments, job = posted(tmp_path, monkeypatch, github)
    [(number, first)] = github.comments.items()
    assert first.count(report.PENDING) == len(attachments)
    assert './run/' not in first
    report.publish(job)
    assert list(github.comments) == [number]
    final = github.comments[number]
    assert report.PENDING not in final and './run/' not in final
    assert final.count('https://github.com/user-attachments/assets/') == len(attachments)
    assert github.uploads == [0, 50, 50, 20]
    assert not job.exists()
    assert not report.log_of(job).exists()


def test_a_failed_upload_leaves_one_short_note_and_no_pending_mark(tmp_path, monkeypatch):
    github = FakeGitHub(fail_uploads=True)
    attachments, job = posted(tmp_path, monkeypatch, github, scenarios=150)
    pages = len(github.comments)
    assert pages > 1
    with pytest.raises(RuntimeError):
        report.publish(job)
    assert len(github.comments) == pages
    log = report.log_of(job)
    for body in github.comments.values():
        assert report.PENDING not in body and '--attach' not in body
        assert body.count('attachments failed to upload') == 1
        assert f'see `./{log.relative_to(tmp_path / "root")}`' in body
        assert len(body) <= report.BODY_LIMIT
    assert sum(body.count(report.FAILED) for body in github.comments.values()) == len(attachments) + pages
    assert not job.exists(), 'the copies of a failed job stay behind'
    assert log.exists()


def test_a_page_that_fails_to_post_leaves_no_earlier_page_pending(tmp_path, monkeypatch):
    github = FakeGitHub(fail_page=2)
    root = tmp_path / 'root'
    for i in range(150):
        scenario(root / 'run', f'scenario_{i:03}', checkpoints=4)
    markdown, attachments = report.summary(report.load(root / 'run'), root, 'Sim', 'abc')
    monkeypatch.setattr(report, 'ROOT', root)
    monkeypatch.setattr(report, 'PUBLISH', root / 'publish')
    monkeypatch.setattr(report, 'gh', github)
    monkeypatch.setattr(report.subprocess, 'Popen', lambda *args, **kwargs: pytest.fail('nothing to upload'))
    with pytest.raises(RuntimeError):
        report.post(7, markdown, attachments)
    [first] = github.comments.values()
    assert report.PENDING not in first and 'attachments failed to upload' in first
    [log] = (root / 'publish').iterdir()
    assert log.suffix == '.log' and 'posting the page failed' in log.read_text()


def test_gh_failures_carry_what_gh_said_not_the_command(monkeypatch):
    def run(command, **kwargs):
        return report.subprocess.CompletedProcess(command, 1, '', 'HTTP 422: Body is too long\n')

    monkeypatch.setattr(report.subprocess, 'run', run)
    command = ['gh', 'pr', 'comment', '7', '--body-file', '-', *['--attach', './a/0.png'] * 50]
    with pytest.raises(RuntimeError) as error:
        report.gh(command)
    assert 'HTTP 422: Body is too long' in str(error.value)
    assert '--attach' not in str(error.value)


def test_a_long_failure_is_cut_in_the_comment():
    fence = report.fenced('x' * 100_000)
    assert len(fence[1]) < report.FAILURE_LIMIT + 100


def test_pending_media_show_as_a_mark_not_a_broken_image():
    body = './a/video.mp4\n![video](./a/v.gif)\n| <img src="./a/01-x.png" width="300"> |\n![chart](./a/chart.png)'
    marked = report.replace_media(body, ['./a/video.mp4', './a/v.gif', './a/01-x.png', './a/chart.png'])
    assert marked == '⏳\n⏳\n| ⏳ |\n⏳'


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
