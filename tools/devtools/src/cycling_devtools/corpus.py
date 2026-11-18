"""`cycling corpus`: real rides from the private corpus replayed under this checkout's build.

The corpus is every trace under the primary checkout's .local/ride-trace/, and .local/traces/ where `cycling
trace pull` puts them, in this build's format (a replay's own replayed.trace left out), newest first. Each replays as a changed build, from power-on to its end. Real rides
cannot be recorded again, so a stop, where this build asks something the ride holds no answer for, is printed for
the review to account for and never fails. A panic or a broken invariant fails. A checkout without the corpus
skips and says so.

Tier 1 replays the 10 newest (`--last 10`); tier 2 (`cycling verify --deep`) replays them all. `--list` prints
the traces it would replay, and `--list-fixtures` the files of the private fixtures `cycling test-rust` replays:
`cycling verify` keys a cached pass on their contents.
"""

import argparse
import json
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Literal

from cycling_devtools import upgrade, workspace


@dataclass
class Replayed:
    trace: str
    verdict: Literal['passed', 'stopped', 'broken', 'not replayed']
    detail: str = ''


def corpus(root):
    local = workspace.main_checkout(root) / '.local'
    return [local / 'ride-trace', local / 'traces']


def traces(directories, target):
    found = []
    for path in (path for directory in directories if directory.is_dir() for path in directory.rglob('*.trace')):
        if path.name == 'replayed.trace':
            continue
        try:
            if upgrade.header(path)[0] == target:
                found.append(path)
        except ValueError:
            continue
    return sorted(found, key=lambda path: path.stat().st_mtime, reverse=True)


def replay_json(trace, out, root):
    done = subprocess.run(
        [
            'bash', str(root / 'tools/simulator/trace.sh'), 'replay', str(trace), '--mode', 'changed',
            '--to', '99999:00', '--every', '99999:00', '--no-bundle', '--out', str(out), '--json',
        ],
        cwd=root,
        capture_output=True,
        text=True,
    )  # fmt: skip
    try:
        return json.loads(done.stdout)
    except json.JSONDecodeError:
        return {'error': done.stderr.strip() or 'the replay printed no JSON'}


def judged(trace, document):
    if 'error' in document:
        return Replayed(str(trace), 'not replayed', document['error'])
    stopped = document.get('stopped')
    if stopped is None:
        return Replayed(str(trace), 'passed', (document.get('divergence') or {}).get('verdict', ''))
    kind = 'broken' if stopped['kind'] == 'Panicked' else 'stopped'
    return Replayed(str(trace), kind, stopped['detail'])


def chosen(root, last):
    found = traces(corpus(root), upgrade.current_format(root))
    return found if last is None else found[:last]


def private_fixtures(root):
    fixtures = workspace.main_checkout(root) / '.local' / 'fixtures'
    return sorted(path for path in fixtures.glob('*/*') if path.name in ('slice.trace', 'expect.toml'))


def run(root=workspace.ROOT, last=10, replay=replay_json, jobs=4):
    target = upgrade.current_format(root)
    directories = corpus(root)
    chosen_traces = chosen(root, last)
    if not chosen_traces:
        return directories, target, []
    with tempfile.TemporaryDirectory(prefix='corpus-') as scratch:

        def one(indexed):
            index, trace = indexed
            return judged(trace, replay(trace, Path(scratch) / str(index), root))

        with ThreadPoolExecutor(max_workers=jobs) as pool:
            return directories, target, list(pool.map(one, enumerate(chosen_traces)))


def main(argv=None):
    parser = argparse.ArgumentParser(prog='cycling corpus', description=__doc__)
    which = parser.add_mutually_exclusive_group()
    which.add_argument('--last', type=int, default=10)
    which.add_argument('--all', action='store_true')
    parser.add_argument('--json', action='store_true')
    listing = parser.add_mutually_exclusive_group()
    listing.add_argument('--list', action='store_true', help='print the traces it would replay, and replay none')
    listing.add_argument('--list-fixtures', action='store_true', help="print the private fixtures' files")
    args = parser.parse_args(argv)
    last = None if args.all else args.last
    if args.list or args.list_fixtures:
        listed = private_fixtures(workspace.ROOT) if args.list_fixtures else chosen(workspace.ROOT, last)
        print(*listed, sep='\n')
        return 0
    directories, target, results = run(last=last, replay=replay_json)
    broken = [result for result in results if result.verdict == 'broken']
    if args.json:
        corpus_dirs = [str(directory) for directory in directories]
        print(json.dumps({'corpus': corpus_dirs, 'format': target, 'replayed': [asdict(r) for r in results]}))
    elif not results:
        print(f'corpus: no format-{target} trace in {" or ".join(map(str, directories))}; skipped')
    else:
        for result in results:
            print(f'{result.verdict:<12} {result.trace}' + (f'\n             {result.detail}' if result.detail else ''))
        print(f'corpus: {len(results)} replayed, {len(broken)} broken')
    return 1 if broken else 0


if __name__ == '__main__':
    sys.exit(main())
