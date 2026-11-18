"""`cycling trace upgrade <trace>...`: bring private corpus traces to this checkout's trace format.

The trace tool of the build that recorded a trace dumps it (`trace dump`, built by twin.py from the build's
archive in the primary checkout's .local/builds/<hash>/), and this checkout's trace tool re-encodes the dump
(`trace undump`). The upgraded trace goes beside the original as <stem>.v<format>.trace, and the original stays.

A trace already in this format is left as it is. One whose build is not archived here, or whose trace tool has
no dump, is unreadable and left untouched. `--patch FILE` applies a backport of the dump on top of the recorded
build. A record this build cannot carry ends the upgraded trace before that record's pass, and is reported; when
it is the first record, nothing is written.
"""

import argparse
import json
import re
import subprocess
import sys
import tempfile
from dataclasses import asdict, dataclass
from pathlib import Path

from cycling_devtools import build_hash, twin, workspace

VERSION = re.compile(r'^pub const VERSION: u8 = (\d+);$', re.MULTILINE)
FIRST_WORLD_LOG_FORMAT = 6
HEADER_LEN = 23
FORMAT_BYTE = 2
BUILD_HASH_BYTES = slice(3, HEADER_LEN)


@dataclass
class Upgraded:
    trace: str
    format: int
    outcome: str
    upgraded: str | None = None
    records: int | None = None
    stopped: dict | None = None


def current_format(root):
    return int(VERSION.search((root / 'packages/services/src/trace.rs').read_text()).group(1))


def header(path):
    """The format and build hash a trace's header names: its first block's payload opens with both."""
    head = path.read_bytes()[:HEADER_LEN]
    if len(head) < HEADER_LEN:
        raise ValueError(f'{path}: too short for a trace header')
    return head[FORMAT_BYTE], head[BUILD_HASH_BYTES].hex()


def recorded_tool(build, patch, root):
    """The trace tool of archived `build`, its archived patch and then `patch` applied, which must dump."""
    archived = build_hash.builds_dir(root) / build
    try:
        commit = (archived / 'commit').read_text().strip()
    except OSError:
        raise twin.NotArchived(f'build {build[:12]} is not archived on this machine') from None
    patch_file = archived / 'diff.patch'
    combined = (patch_file.read_bytes() if patch_file.exists() else b'') + (patch or b'')
    return twin.binary(commit, combined or None, root=root, needs='dump')


def undump(dump, out, root):
    done = subprocess.run(
        ['bash', str(root / 'tools/simulator/trace.sh'), 'trace', 'undump', str(dump), '--out', str(out), '--json'],
        cwd=root,
        capture_output=True,
        text=True,
    )
    try:
        return json.loads(done.stdout)
    except json.JSONDecodeError:
        return {'error': done.stderr.strip() or 'trace undump printed no JSON'}


def upgrade(path, *, root=workspace.ROOT, patch=None, tool=recorded_tool, reencode=undump):
    target = current_format(root)
    version, build = header(path)
    if version == target:
        return Upgraded(str(path), version, f'already format {target}')
    if version < FIRST_WORLD_LOG_FORMAT:
        return Upgraded(str(path), version, f'unreadable, left untouched: format {version} predates the world log')
    try:
        old = tool(build, patch, root)
    except (twin.NotArchived, twin.Incapable) as error:
        return Upgraded(str(path), version, f'unreadable, left untouched: {error}')
    out = path.with_name(f'{path.stem}.v{target}.trace')
    with tempfile.NamedTemporaryFile(prefix='dump-', suffix='.jsonl') as dump:
        dumped = subprocess.run([str(old), 'dump', str(path)], stdout=dump, stderr=subprocess.PIPE, text=True)
        if dumped.returncode != 0:
            return Upgraded(str(path), version, f'unreadable, left untouched: {dumped.stderr.strip()}')
        dump.flush()
        result = reencode(Path(dump.name), out, root)
    if 'error' in result:
        out.unlink(missing_ok=True)
        return Upgraded(str(path), version, f'not upgraded: {result["error"]}')
    stopped = result.get('stopped')
    if not result.get('records'):
        out.unlink(missing_ok=True)
        return Upgraded(
            str(path), version, 'not upgraded, left untouched: its first record cannot be carried', None, 0, stopped
        )
    outcome = f'upgraded to format {target}' if stopped is None else f'upgraded to format {target}, cut short'
    return Upgraded(str(path), version, outcome, str(out), result.get('records'), stopped)


def described(found):
    line = f'{found.trace}: format {found.format}, {found.outcome}'
    if found.upgraded:
        line += f'\n  {found.records} records in {found.upgraded}'
    if found.stopped:
        stopped = found.stopped
        line += f'\n  ends before its pass at {stopped["at_ms"]} ms: {stopped["record"]}: {stopped["why"]}'
    return line


def main(argv=None):
    parser = argparse.ArgumentParser(prog='cycling trace upgrade', description=__doc__)
    parser.add_argument('traces', nargs='+', type=Path)
    parser.add_argument('--patch', type=Path, help='a backport of trace dump for builds that predate it')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    patch = args.patch.read_bytes() if args.patch else None
    found = [upgrade(path.resolve(), patch=patch) for path in args.traces]
    if args.json:
        print(json.dumps({'upgraded': [asdict(item) for item in found]}))
    else:
        print('\n'.join(described(item) for item in found))
    return 0


if __name__ == '__main__':
    sys.exit(main())
