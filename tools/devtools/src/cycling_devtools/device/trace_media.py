"""Pull a ride trace with what its header needs to rebuild the media it started on.

`cycling trace pull` copies the trace from STORE, asks `cycling trace media`
what its header names, and reads those media back through read-only paths
only: ride export for the internal journal and RIDES, up to the ends the
header recorded, and `files get` for each file the header lists. A local
terrain pack under .local/ with the header's SHA-256 stands in for TERRAIN.
`cycling trace rebuild` then writes <trace>.origin beside the trace and says
what it could not rebuild. Nothing is written to the device.

The pulled media go into <trace stem>.media/ beside the trace:
internal.slots, rides.slots and rides.epoch, files.list and files/<name>,
and terrain.pack, a link to the local pack.
"""

import hashlib
import shlex
import shutil
import subprocess
from pathlib import Path

from cycling_devtools.cli import TRACE
from cycling_devtools.device import volumes
from cycling_devtools.rides import export
from cycling_devtools.workspace import DEVICE_LOCAL, ROOT

SLOT_SIZE = export.SLOT_SIZE
PACK_ROOTS = (ROOT / '.local' / 'terrain', DEVICE_LOCAL / 'terrain')


def trace_tool(*args):
    """Run `cycling trace` and return what it printed."""
    command = [*shlex.split(TRACE), *map(str, args)]
    done = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if done.returncode != 0:
        raise RuntimeError(f'cycling trace {args[0]}: {done.stderr.strip()}')
    return done.stdout


def header_media(text):
    """`cycling trace media` lines as a dict; a medium without anything to pull is None."""
    media = {'journal': None, 'rides': None, 'files': None, 'terrain': None}
    for line in text.splitlines():
        words = line.split()
        if not words or words[1:] == ['none']:
            continue
        kind = words[0]
        if kind == 'journal':
            media['journal'] = int(words[1])
        elif kind == 'rides' and words[1] in ('log', 'uncertain'):
            media['rides'] = {'store': words[2], 'head': int(words[3])}
        elif kind == 'files':
            media['files'] = {'store': words[1], 'generation': int(words[2]), 'entries': []}
        elif kind == 'file':
            name, length, checksum, revision = words[1:5]
            media['files']['entries'].append(
                {'name': name, 'length': int(length), 'crc32': checksum, 'revision': int(revision)}
            )
        elif kind == 'terrain':
            media['terrain'] = {'length': int(words[1]), 'sha256': words[2], 'tag': ' '.join(words[4:])}
    return media


def find_pack(length, sha256, roots=PACK_ROOTS):
    """The local pack of `length` bytes whose SHA-256 is `sha256`, or None."""
    seen = set()
    for root in roots:
        if not root.is_dir():
            continue
        for path in sorted(root.rglob('*')):
            if not path.is_file() or path.stat().st_size != length or path.resolve() in seen:
                continue
            seen.add(path.resolve())
            digest = hashlib.sha256()
            with path.open('rb') as source:
                while chunk := source.read(1 << 20):
                    digest.update(chunk)
            if digest.hexdigest() == sha256:
                return path.resolve()
    return None


def safe_name(name):
    """A FILES name as a path under files/, refusing one that would leave it."""
    path = Path(name)
    if path.is_absolute() or '..' in path.parts or not path.parts:
        raise ValueError(f'refusing file name {name!r}')
    return path


def pull_slots(connection, medium, count, output):
    """The first `count` slots of `medium` into `output`; the indices of invalid containers."""
    partial = output.with_name(output.name + '.partial')
    invalid = export.download_prefix(connection, partial, count, medium)
    partial.replace(output)
    return invalid


def pull_journals(connection, media, out, problems):
    if media['journal'] is not None:
        try:
            upper = export.info(connection, 'internal')['upper_bound']
            pull_slots(connection, 'internal', min(upper, media['journal']), out / 'internal.slots')
        except (RuntimeError, ValueError, TimeoutError) as error:
            problems.append(f'internal ride journal: {error}')
    if media['rides'] is not None:
        try:
            info = export.info(connection, 'bulk')
            slots = out / 'rides.slots'
            invalid = []
            if int(info['store_id'], 16) == int(media['rides']['store'], 16):
                invalid = pull_slots(connection, 'bulk', min(info['upper_bound'], media['rides']['head']), slots)
            else:
                slots.write_bytes(b'')
            epoch = (
                f'store {info["store_id"]}\ngeneration {info["generation"]}\ninvalid {" ".join(map(str, invalid))}\n'
            )
            (out / 'rides.epoch').write_text(epoch)
        except (RuntimeError, ValueError, TimeoutError) as error:
            problems.append(f'RIDES: {error}')


def pull_file(transfer, name, path):
    """The revision whose bytes `files get` wrote to `path`; None when the file is gone."""
    try:
        stat = volumes.files_get(transfer, name, path)
    except volumes.TransferError as error:
        if error.status == 'not_found':
            return None
        raise
    _, _, _, after = transfer.call('files_stat', payload=name.encode(), allow=('not_found',))
    if not after or volumes.decode_entry(after)['revision'] != stat['revision']:
        path.unlink()
        raise RuntimeError(f'written again while it was read (revision {stat["revision"]})')
    return stat['revision']


def pull_files(connection, media, out, problems):
    if media['files'] is None:
        return
    transfer = volumes.open_session(connection)
    try:
        status, _, _, data = transfer.call('files_status', allow=('uninitialized', 'foreign', 'corrupt'))
        if status != 'ok':
            problems.append(f'FILES: {status}')
            return
        lines = [f'store {int.from_bytes(data[8:16], "little"):016x}']
        for entry in media['files']['entries']:
            name = entry['name']
            path = out / 'files' / safe_name(name)
            path.parent.mkdir(parents=True, exist_ok=True)
            try:
                revision = pull_file(transfer, name, path)
            except (RuntimeError, ValueError, TimeoutError) as error:
                problems.append(f'file {name}: {error}')
                lines.append(f'unread {name}')
                continue
            if revision is not None:
                lines.append(f'file {revision} {name}')
        (out / 'files.list').write_text('\n'.join(lines) + '\n')
    finally:
        transfer.call('end')


def pull(connection, name=None, directory=Path('.local/traces'), tool=trace_tool, roots=PACK_ROOTS):
    """Pull the trace `name` (the newest by default) and rebuild the media it started on beside it."""
    transfer = volumes.open_session(connection)
    try:
        result = volumes.trace_pull(transfer, name, directory)
    finally:
        transfer.call('end')
    trace = Path(result['path'])
    media = header_media(tool('media', trace))
    out = trace.with_name(trace.stem + '.media')
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(mode=0o700)
    problems = []
    pull_journals(connection, media, out, problems)
    pull_files(connection, media, out, problems)
    if media['terrain'] is not None:
        pack = find_pack(media['terrain']['length'], media['terrain']['sha256'], roots)
        if pack is not None:
            (out / 'terrain.pack').symlink_to(pack)
    report = tool('rebuild', trace, '--pulled', out).splitlines()
    result.update(media=str(out), origin=report[0].removeprefix('origin '), rebuilt=report[1:])
    if problems:
        result['problems'] = problems
    return result
