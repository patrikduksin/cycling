"""Summarize a simulator run directory and post it to a pull request.

Each scenario directory holds `result.json` and the artifacts the simulator wrote
(`tools/simulator/src/recording.rs`). This module writes `summary.md` for a PR
comment, `index.html` for local browsing, and attaches media with `gh`, which
rewrites local references to the uploaded files.
"""

import argparse
import html
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import traceback
from contextlib import suppress
from pathlib import Path

from cycling_devtools.workspace import ROOT

# gh accepts at most this many attachments per command.
ATTACHMENT_LIMIT = 50
# GitHub refuses a longer comment body.
BODY_LIMIT = 65_536
# Room kept in every comment for the note a failed upload adds.
NOTE_ROOM = 500
# A failure longer than this is cut; result.json keeps all of it.
FAILURE_LIMIT = 4_000
CONTINUED = '## Simulator scenarios (continued)\n'
CHECKPOINT_LIMIT = 4
ASSET = r'https://github\.com/user-attachments/assets/[0-9a-fA-F-]+'
ASSET_URL_LENGTH = len('https://github.com/user-attachments/assets/') + 36
PENDING = '⏳'
FAILED = '✖'
PUBLISH = ROOT / '.local/sim/publish'
# The listing labels attachment i as `![i](path)`; gh renders it either as
# Markdown or as an <img> tag, with attributes in any order.
LISTED = (
    re.compile(rf'!\[(\d+)\]\(({ASSET})\)'),
    re.compile(rf'<img\b[^>]*?alt="(\d+)"[^>]*?src="({ASSET})"'),
    re.compile(rf'<img\b[^>]*?src="({ASSET})"[^>]*?alt="(\d+)"'),
)
TEST_LINE = re.compile(r'^test (?:\S+::)?(\w+) \.\.\. (ok|FAILED|ignored)', re.MULTILINE)


def load(run):
    """Scenario results, checked against cargo's log in `cargo.log` when present
    so aborted runs and scenarios that wrote nothing still count as failures."""
    run = Path(run)
    results = {}
    for path in sorted(run.glob('*/result.json')):
        result = json.loads(path.read_text())
        result['dir'] = path.parent
        results[path.parent.name] = result
    log = run / 'cargo.log'
    if log.exists():
        text = log.read_text(errors='replace')
        for name, verdict in TEST_LINE.findall(text):
            result = results.get(name)
            if verdict == 'ignored':
                continue
            if result is None:
                results[name] = missing(run, name, 'The scenario wrote no artifacts.')
            elif verdict == 'FAILED' and result['status'] == 'pass':
                result['status'] = 'fail'
                result['failure'] = 'cargo reported this test as failed after its simulator was dropped.'
        if 'test result:' not in text:
            results['cargo_test'] = missing(run, 'cargo_test', 'cargo test did not finish; see cargo.log.')
    ordered = sorted(results.values(), key=lambda r: (r['status'] == 'pass', r['name']))
    return ordered


def missing(run, name, failure):
    return {
        'name': name,
        'status': 'fail',
        'failure': failure,
        'simulated_ms': 0,
        'frames': 0,
        'files': [],
        'rides': [],
        'dir': run / name,
    }


def fenced(text):
    """A code block that neither closes early nor splits comments."""
    text = text.strip()
    if len(text) > FAILURE_LIMIT:
        text = text[:FAILURE_LIMIT] + '\n… (cut; result.json has the whole failure)'
    text = text.replace('\n### ', '\n ### ')
    fence = '`' * max([3, *(len(m) + 1 for m in re.findall(r'`{3,}', text))])
    return [fence + 'text', text, fence]


def title(name):
    return name.replace('_', ' ').capitalize()


def clock(ms):
    seconds = ms // 1000
    if seconds >= 3600:
        return f'{seconds // 3600}:{seconds // 60 % 60:02}:{seconds % 60:02}'
    return f'{seconds // 60}:{seconds % 60:02}'


def reference(path, base):
    """A path gh can match between the comment body and its attachment list."""
    return './' + os.path.relpath(path, base)


def rides_text(result):
    rides = result.get('rides', [])
    if not rides:
        return '—'
    return ', '.join(f'#{r["id"]}: {r["samples"]} samples, {r["track_m"] / 1000:.2f} km' for r in rides)


def video(result):
    return next((f for f in result['files'] if f.startswith('video.')), None)


def media(result):
    """Video first, then checkpoints and the chart, in the order a reviewer reads."""
    files = result['files']
    checkpoints = [f for f in files if f[:2].isdigit() and f.endswith('.png')]
    if len(checkpoints) > CHECKPOINT_LIMIT:
        checkpoints = [*checkpoints[: CHECKPOINT_LIMIT - 1], checkpoints[-1]]
    if result['status'] != 'pass' or not checkpoints:
        checkpoints.append('final.png')
    chart = 'chart.png' if 'chart.png' in files else None
    return video(result), checkpoints, chart


def summary(results, base, heading, commit):
    """Markdown for one PR comment and the files it references."""
    passed = sum(r['status'] == 'pass' for r in results)
    simulated = sum(r['simulated_ms'] for r in results)
    icon = '✅' if passed == len(results) else '❌'
    lines = [
        f'## {icon} {heading}: {passed}/{len(results)} scenarios passed',
        '',
        f'VANA on a simulated C606 at `{commit}` · {clock(simulated)} of simulated time. '
        'Videos compress quiet stretches and play actions in real time.',
        '',
        '| | Scenario | Simulated | Rides recorded |',
        '|---|---|---|---|',
    ]
    for r in results:
        mark = '✅' if r['status'] == 'pass' else '❌'
        lines.append(f'| {mark} | {title(r["name"])} | {clock(r["simulated_ms"])} | {rides_text(r)} |')
    attachments = []
    for r in results:
        mark = '✅' if r['status'] == 'pass' else '❌'
        lines += ['', f'### {mark} {title(r["name"])}', '']
        if r['failure']:
            lines += [*fenced(r['failure']), '']
        video, checkpoints, chart = media(r)
        if video:
            path = reference(r['dir'] / video, base)
            attachments.append(path)
            # A bare video URL renders as a player; a GIF needs image syntax.
            lines += [path if video.endswith('.mp4') else f'![video]({path})', '']
        images = [(name, reference(r['dir'] / name, base)) for name in checkpoints]
        if images:
            lines.append('| ' + ' | '.join(caption(name) for name, _ in images) + ' |')
            lines.append('|' + '---|' * len(images))
            lines.append('| ' + ' | '.join(f'<img src="{path}" width="300">' for _, path in images) + ' |')
            attachments += [path for _, path in images]
        if chart:
            path = reference(r['dir'] / chart, base)
            attachments.append(path)
            lines += [
                '',
                '<details><summary>World over time</summary>',
                '',
                f'![World over time]({path})',
                '',
                '</details>',
            ]
    lines += [
        '',
        '<sub>Generated by `cycling sim-pr`. Simulation does not cover DMA, display timing or radio coexistence.</sub>',
    ]
    return '\n'.join(lines) + '\n', attachments


def caption(name):
    stem = Path(name).stem
    if stem[:2].isdigit():
        stem = stem[3:]
    return stem.replace('-', ' ')


def page(results, heading):
    """A local HTML index of every artifact, for browsing a run."""
    passed = sum(r['status'] == 'pass' for r in results)
    parts = [
        '<!doctype html><meta charset="utf-8">',
        f'<title>{html.escape(heading)}</title>',
        '<style>body{font:14px system-ui;background:#14181d;color:#c9d1d9;margin:2em}'
        'img,video{max-width:100%;border:1px solid #2b323a}section{margin:2em 0}'
        '.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:12px}'
        '.fail{color:#f85149}.pass{color:#3fb950}pre{background:#0d1117;padding:1em;overflow:auto}'
        'a{color:#00d7d7}</style>',
        f'<h1>{html.escape(heading)}: {passed}/{len(results)} passed</h1>',
    ]
    for r in results:
        name = r['dir'].name
        status = 'pass' if r['status'] == 'pass' else 'fail'
        parts.append(f'<section><h2 class="{status}">{html.escape(title(r["name"]))}</h2>')
        parts.append(
            f'<p>{clock(r["simulated_ms"])} simulated · {r["frames"]} frames · rides: {html.escape(rides_text(r))}</p>'
        )
        if r['failure']:
            parts.append(f'<pre class="fail">{html.escape(r["failure"])}</pre>')
        for f in r['files']:
            if f.startswith('video.'):
                parts.append(
                    f'<video controls src="{name}/{f}"></video>' if f.endswith('.mp4') else f'<img src="{name}/{f}">'
                )
        parts.append('<div class="grid">')
        for f in r['files']:
            if f.endswith('.png'):
                parts.append(
                    f'<figure><img src="{name}/{f}"><figcaption>{html.escape(caption(f))}</figcaption></figure>'
                )
        parts.append('</div>')
        extras = sorted(
            p.name
            for p in (r['dir'].iterdir() if r['dir'].is_dir() else [])
            if p.suffix in ('.gpx', '.json', '.svg', '.txt')
        )
        parts.append('<p>' + ' · '.join(f'<a href="{name}/{e}">{e}</a>' for e in extras) + '</p></section>')
    return '\n'.join(parts) + '\n'


def comments(markdown, attachments):
    """Comment bodies, each with the attachments it shows, that stay under
    GitHub's limit once every attachment is an asset URL. Comments break
    between scenarios, and a scenario too long for one comment breaks between
    its table rows, repeating its heading and table header."""
    limit = BODY_LIMIT - NOTE_ROOM

    def uploaded_size(text):
        return len(text) + sum(text.count(a) * (ASSET_URL_LENGTH - len(a)) for a in attachments)

    head, *sections = markdown.split('\n### ')
    pieces = [
        piece for section in sections for piece in by_rows('\n### ' + section, limit - len(CONTINUED), uploaded_size)
    ]
    chunks, body, used = [], head, uploaded_size(head)
    for piece, size in pieces:
        if used + size > limit and body.strip():
            chunks.append(body)
            body, used = CONTINUED, len(CONTINUED)
        body += piece
        used += size
    chunks.append(body)
    return [(chunk, [a for a in attachments if a in chunk]) for chunk in chunks]


def by_rows(section, limit, uploaded_size):
    """(text, uploaded size) pieces of a section, each within `limit` unless
    its heading and table header alone exceed it."""
    lines = section.split('\n')
    sizes = [uploaded_size(line) + 1 for line in lines]
    table = next((i + 1 for i, line in enumerate(lines) if line.startswith('|---')), len(lines))
    if sum(sizes) <= limit or table == len(lines):
        return [(section, sum(sizes) - 1)]
    again = ['', f'{lines[1]} (continued)', '', *lines[table - 2 : table]]
    again_size = sum(len(line) + 1 for line in again)
    pieces, piece, used = [], lines[:table], sum(sizes[:table])
    for line, size in zip(lines[table:], sizes[table:], strict=True):
        if used + size > limit:
            pieces.append(piece)
            piece, used = list(again), again_size
        piece.append(line)
        used += size
    pieces.append(piece)
    return [('\n'.join(piece), sum(uploaded_size(line) + 1 for line in piece) - 1) for piece in pieces]


def replace_media(body, files, mark=PENDING):
    for path in map(re.escape, files):
        body = re.sub(rf'<img src="{path}"[^>]*>|!\[[^\]]*\]\({path}\)|^{path}$', mark, body, flags=re.MULTILINE)
    return body


def failed(body, files, log):
    """The body with `files` marked as failed and one note under its heading."""
    heading, rest = replace_media(body, files, FAILED).split('\n', 1)
    note = (
        f'{FAILED} {len(files)} attachments failed to upload; see `{reference(log, ROOT)}` on the machine that posted.'
    )
    return f'{heading}\n\n{note}\n{rest}'


def give_up(pages, log):
    """Mark each page's media that never uploaded as failed; `log` says why.
    A page whose edit fails too is skipped, so it cannot hide the others."""
    for page in pages:
        if page['files']:
            with suppress(Exception):
                edit(page['comment'], failed(page['body'], page['files'], log))


def snapshot(attachments):
    """Copies of the attachments for the background upload, which a later run
    cannot overwrite; returns the job directory and each copy by original."""
    PUBLISH.mkdir(parents=True, exist_ok=True)
    job = Path(tempfile.mkdtemp(dir=PUBLISH))
    copies = {}
    for i, path in enumerate(attachments):
        copy = job / f'{i}{Path(path).suffix}'
        shutil.copyfile(ROOT / path, copy)
        copies[path] = reference(copy, ROOT)
    return job, copies


def log_of(job):
    """Beside the job, so it outlives the copies a finished job deletes."""
    return job.parent / f'{job.name}.log'


def post(pr, markdown, attachments):
    job, copies = snapshot(attachments)
    log = log_of(job)
    pages = []
    try:
        for body, files in comments(markdown, attachments):
            pages.append({'comment': post_comment(pr, replace_media(body, files)), 'body': body, 'files': files})
    except Exception:
        log.write_text(traceback.format_exc())
        give_up(pages, log)
        shutil.rmtree(job)
        raise
    (job / 'job.json').write_text(json.dumps({'pr': str(pr), 'pages': pages, 'copies': copies}))
    with log.open('w') as out:
        subprocess.Popen(
            [sys.executable, '-m', 'cycling_devtools.sim.report', '--publish', str(job)],
            cwd=ROOT,
            stdout=out,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    print(f'posted on #{pr}; {len(attachments)} attachments upload in the background, log in {log}')


def publish(job):
    """gh rewrites only Markdown references, never an HTML <img>, so each batch
    goes up in a throwaway comment that lists it as Markdown images, and the
    uploaded URLs are read back from that comment before it is deleted."""
    state = json.loads((job / 'job.json').read_text())
    pages, copies = state['pages'], state['copies']
    remaining = list(copies)
    log = log_of(job)
    try:
        while remaining:
            batch, later = remaining[:ATTACHMENT_LIMIT], remaining[ATTACHMENT_LIMIT:]
            listing = 'Uploading simulator artifacts…\n\n' + '\n\n'.join(
                f'![{i}]({copies[path]})' for i, path in enumerate(batch)
            )
            upload = post_comment(state['pr'], listing, [copies[path] for path in batch])
            try:
                uploaded = gh(['gh', 'api', upload, '--jq', '.body'])
            finally:
                gh(['gh', 'api', '-X', 'DELETE', upload])
            for page in pages:
                if any(path in page['files'] for path in batch):
                    page['body'] = resolve(page['body'], batch, uploaded)
                    page['files'] = [path for path in page['files'] if path not in batch]
                    edit(page['comment'], replace_media(page['body'], later))
            remaining = later
    except Exception:
        give_up(pages, log)
        raise
    finally:
        shutil.rmtree(job)
    log.unlink()


def post_comment(pr, body, attachments=()):
    command = ['gh', 'pr', 'comment', str(pr), '--body-file', '-']
    for path in attachments:
        command += ['--attach', path]
    url = gh(command, body)
    number = url.strip().rsplit('#issuecomment-', 1)[-1]
    if not number.isdigit():
        raise RuntimeError(f'gh did not report the new comment: {url!r}')
    return f'repos/{{owner}}/{{repo}}/issues/comments/{number}'


def edit(endpoint, body):
    gh(['gh', 'api', '-X', 'PATCH', endpoint, '-F', 'body=@-'], body)


def gh(command, stdin=None):
    """gh's output; on failure, an error with what gh printed and not the
    command, whose attachment paths would fill a comment."""
    done = subprocess.run(command, input=stdin, text=True, cwd=ROOT, capture_output=True)
    if done.returncode:
        raise RuntimeError(f'{" ".join(command[:3])} exited with status {done.returncode}: {done.stderr.strip()}')
    return done.stdout


def resolve(body, files, uploaded):
    """Replace local paths with the asset URLs gh uploaded, matched by listing index."""
    urls = {}
    for pattern in LISTED:
        for match in pattern.finditer(uploaded):
            index, url = match.groups() if match.group(1).isdigit() else reversed(match.groups())
            urls.setdefault(int(index), url)
    # gh turns videos into bare URLs without their label; those fill the
    # remaining indices in listing order.
    bare = [url for url in re.findall(ASSET, uploaded) if url not in urls.values()]
    unlabelled = [i for i in range(len(files)) if i not in urls]
    if len(bare) != len(unlabelled):
        raise RuntimeError(f'uploaded {len(files) - len(unlabelled) + len(bare)} of {len(files)} attachments')
    urls.update(zip(unlabelled, bare, strict=True))
    for i, path in enumerate(files):
        body = body.replace(path, urls[i])
    return body


def commit():
    try:
        return subprocess.run(
            ['git', 'rev-parse', '--short', 'HEAD'], cwd=ROOT, capture_output=True, text=True, check=True
        ).stdout.strip()
    except OSError, subprocess.CalledProcessError:
        return 'unknown'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('run', type=Path, nargs='?', help='directory given to SIM_ARTIFACTS')
    parser.add_argument('--pr', help='pull request to comment on with inline artifacts')
    parser.add_argument('--heading', default='Simulator scenarios')
    parser.add_argument('--publish', type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.publish:
        publish(args.publish)
        return 0
    if args.run is None:
        parser.error('name the run directory')
    results = load(args.run)
    if not results:
        print(f'no scenario results under {args.run}', file=sys.stderr)
        return 1
    markdown, attachments = summary(results, ROOT, args.heading, commit())
    (args.run / 'summary.md').write_text(markdown)
    (args.run / 'index.html').write_text(page(results, args.heading))
    failed = [r['name'] for r in results if r['status'] != 'pass']
    print(f'{len(results) - len(failed)}/{len(results)} passed; report {args.run / "index.html"}')
    if args.pr:
        post(args.pr, markdown, attachments)
    return 1 if failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
