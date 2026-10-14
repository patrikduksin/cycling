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
import subprocess
import sys
from pathlib import Path

from cycling_devtools.workspace import ROOT

# gh accepts at most this many attachments per command.
ATTACHMENT_LIMIT = 50
CHECKPOINT_LIMIT = 4
ASSET = r'https://github\.com/user-attachments/assets/[0-9a-fA-F-]+'
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
    text = text.strip().replace('\n### ', '\n ### ')
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


def media(result):
    """Video first, then checkpoints and the chart, in the order a reviewer reads."""
    files = result['files']
    video = next((f for f in files if f.startswith('video.')), None)
    checkpoints = [f for f in files if f[:2].isdigit() and f.endswith('.png')]
    if len(checkpoints) > CHECKPOINT_LIMIT:
        checkpoints = [*checkpoints[: CHECKPOINT_LIMIT - 1], checkpoints[-1]]
    if result['status'] != 'pass' or not checkpoints:
        checkpoints.append('final.png')
    chart = 'chart.png' if 'chart.png' in files else None
    return video, checkpoints, chart


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
        '<sub>Generated by `mise run sim-pr`. Simulation does not cover DMA, display timing or radio coexistence.</sub>',
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
    """Split into comments that each stay within gh's attachment limit."""
    if len(attachments) <= ATTACHMENT_LIMIT:
        return [(markdown, attachments)]
    head, *sections = markdown.split('\n### ')
    chunks, body, files = [], head, []
    for section in sections:
        section = '\n### ' + section
        needed = [a for a in attachments if a in section]
        if files and len(files) + len(needed) > ATTACHMENT_LIMIT:
            chunks.append((body, files))
            body, files = '## Simulator scenarios (continued)\n', []
        body += section
        files += needed
    chunks.append((body, files))
    return chunks


def post(pr, markdown, attachments):
    """Upload, then lay out. gh rewrites only Markdown image references, so each
    comment first lists its files as plain references, then is edited into the
    report with the uploaded URLs in tables and as video players."""
    for body, files in comments(markdown, attachments):
        listing = 'Uploading simulator artifacts…\n\n' + '\n\n'.join(f'![{i}]({path})' for i, path in enumerate(files))
        command = ['gh', 'pr', 'comment', str(pr), '--body-file', '-']
        for path in files:
            command += ['--attach', path]
        url = gh(command, listing)
        comment = url.strip().rsplit('#issuecomment-', 1)[-1]
        if not comment.isdigit():
            raise RuntimeError(f'gh did not report the new comment: {url!r}')
        endpoint = f'repos/{{owner}}/{{repo}}/issues/comments/{comment}'
        try:
            uploaded = gh(['gh', 'api', endpoint, '--jq', '.body'])
            gh(['gh', 'api', '-X', 'PATCH', endpoint, '-F', 'body=@-'], resolve(body, files, uploaded))
        except Exception:
            # Leave no half-written "Uploading…" comment behind.
            subprocess.run(['gh', 'api', '-X', 'DELETE', endpoint], cwd=ROOT, capture_output=True)
            raise


def gh(command, stdin=None):
    return subprocess.run(command, input=stdin, text=True, cwd=ROOT, check=True, capture_output=True).stdout


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
    parser.add_argument('run', type=Path, help='directory given to SIM_ARTIFACTS')
    parser.add_argument('--pr', help='pull request to comment on with inline artifacts')
    parser.add_argument('--heading', default='Simulator scenarios')
    args = parser.parse_args(argv)
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
