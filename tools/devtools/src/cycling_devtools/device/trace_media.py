"""Pull a ride trace with what its header needs to rebuild the media it started on.

`cycling trace pull` copies the trace from STORE, asks `cycling trace media`
what its header names, and reads those media back through read-only paths
only, up to the ends the header recorded: ride export for the internal
journal, and one transfer session for the RIDES log's records and `files get`
of each file the header lists. A local terrain pack under .local/ with the
header's SHA-256 stands in for TERRAIN. `cycling trace rebuild` then writes
<trace>.origin beside the trace and says what it could not rebuild. Nothing is
written to the device.

The pulled media go into <trace stem>.media/ beside the trace:
internal.slots, rides.slots and rides.epoch, files.list and files/<name>,
and terrain.pack, a link to the local pack. RIDES only grows within an
epoch, so the slots pulled for one are kept in .cache/ beside the traces, and
a later pull reads only the records after them once the device still holds
their last one.
"""

import hashlib
import shlex
import shutil
import subprocess
import time
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
        if len(words) < 2 or words[1] in ('none', 'unusable', 'unreadable'):
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


def pull_internal(connection, media, out, problems):
    if media['journal'] is None:
        return
    try:
        upper = export.info(connection, 'internal')['upper_bound']
        pull_slots(connection, 'internal', min(upper, media['journal']), out / 'internal.slots')
    except (RuntimeError, ValueError, TimeoutError) as error:
        problems.append(f'internal ride journal: {error}')


def rides_log(connection, media, problems):
    """The RIDES log the device holds now: its store id, its generation, and how many of its slots the trace
    started on, none when the log was cleared since. None when the header names no log or the device does not
    answer."""
    if media['rides'] is None:
        return None
    try:
        info = export.info(connection, 'bulk')
    except (RuntimeError, ValueError, TimeoutError) as error:
        problems.append(f'RIDES: {error}')
        return None
    store = int(info['store_id'], 16)
    head = min(info['upper_bound'], media['rides']['head']) if store == int(media['rides']['store'], 16) else 0
    return store, info['generation'], head


def slot_of(record):
    """A record as ride export gives it: its slot, and whether its container was valid."""
    if record is None or len(record) != SLOT_SIZE:
        return bytes(SLOT_SIZE), False
    return record, True


def cached_rides(cache, store):
    """The cached slots of the RIDES epoch `store` and the indices of its invalid ones; nothing when there is no
    readable cache."""
    slots, invalid = cache / f'rides-{store:016x}.slots', cache / f'rides-{store:016x}.invalid'
    try:
        return slots.read_bytes(), {int(word) for word in invalid.read_text().split()}
    except OSError, ValueError:
        return b'', set()


def keep_rides(cache, store, slots, invalid):
    """Cache the slots of the RIDES epoch `store`: the invalid indices first, so a cache cut short between the
    two files lists none past its slots."""
    cache.mkdir(parents=True, exist_ok=True)
    for name, data in (
        (f'rides-{store:016x}.invalid', ' '.join(map(str, sorted(invalid))).encode()),
        (f'rides-{store:016x}.slots', slots),
    ):
        partial = cache / f'{name}.partial'
        partial.write_bytes(data)
        partial.replace(cache / name)


def pull_rides(transfer, store, head, output, cache):
    """The first `head` slots of the RIDES epoch `store` into `output`; the indices of invalid containers.

    A cached prefix of the same epoch is kept when the device still holds the prefix's last slot as cached: an
    epoch's records never change once written, so only the records after it are read."""
    cached, cached_invalid = cached_rides(cache, store)
    kept = min(len(cached) // SLOT_SIZE, head)
    slots, invalid = cached[: kept * SLOT_SIZE], {index for index in cached_invalid if index < kept}
    stale = kept and slot_of(volumes.rides_read(transfer, store, kept - 1, 1)[0]) != (
        slots[-SLOT_SIZE:],
        kept - 1 not in invalid,
    )
    if stale:
        slots, invalid = b'', set()
    first = len(slots) // SLOT_SIZE
    fresh = bytearray()
    for index, record in enumerate(volumes.rides_read(transfer, store, first, head - first), first):
        slot, valid = slot_of(record)
        fresh += slot
        if not valid:
            invalid.add(index)
    slots += fresh
    if stale or len(slots) > len(cached):
        keep_rides(cache, store, slots, invalid)
    output.write_bytes(slots)
    return sorted(invalid)


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


def pull_files(transfer, media, out, problems):
    if media['files'] is None:
        return
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


def pull_media(connection, media, out, cache, problems, timing):
    """RIDES and FILES as the header names them, in one transfer session. RIDES goes through ride export instead
    on firmware from before RIDES_READ."""
    rides = rides_log(connection, media, problems)
    head = rides[2] if rides else 0
    pulled = {}

    def work(transfer):
        if head:
            try:
                pulled['invalid'] = pull_rides(transfer, rides[0], head, out / 'rides.slots', cache)
            except volumes.TransferError as error:
                if error.status == 'invalid':
                    pulled['export'] = True
                else:
                    problems.append(f'RIDES: {error}')
            except (RuntimeError, ValueError, TimeoutError) as error:
                problems.append(f'RIDES: {error}')
        pull_files(transfer, media, out, problems)

    if head or media['files'] is not None:
        session(connection, timing, 'media_session', work)
    if rides is None:
        return
    if not head:
        (out / 'rides.slots').write_bytes(b'')
        pulled['invalid'] = []
    elif pulled.get('export'):
        try:
            pulled['invalid'] = pull_slots(connection, 'bulk', head, out / 'rides.slots')
        except (RuntimeError, ValueError, TimeoutError) as error:
            problems.append(f'RIDES: {error}')
    if 'invalid' in pulled:
        store, generation, _ = rides
        invalid = ' '.join(map(str, pulled['invalid']))
        (out / 'rides.epoch').write_text(f'store {store:016x}\ngeneration {generation}\ninvalid {invalid}\n')


def session(connection, timing, name, work):
    """Run `work(transfer)` in one transfer session, the app paused, and note how long it took here and where its
    time went on the device."""
    started = time.monotonic()
    transfer = volumes.open_session(connection)
    try:
        return work(transfer)
    finally:
        spent = volumes.end(transfer)
        timing[name] = {'s': round(time.monotonic() - started, 3), **spent}


def pull(
    connection,
    name=None,
    directory=Path('.local/traces'),
    tool=trace_tool,
    roots=PACK_ROOTS,
):
    """Pull the trace `name` (the newest by default) and rebuild the media it started on beside it."""
    started, timing = time.monotonic(), {}
    result = session(
        connection, timing, 'trace_session', lambda transfer: volumes.trace_pull(transfer, name, directory)
    )
    trace = Path(result['path'])
    media = header_media(tool('media', trace))
    out = trace.with_name(trace.stem + '.media')
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(mode=0o700)
    problems = []
    lap = time.monotonic()
    pull_internal(connection, media, out, problems)
    timing['internal_s'] = round(time.monotonic() - lap, 3)
    pull_media(connection, media, out, directory / '.cache', problems, timing)
    if media['terrain'] is not None:
        pack = find_pack(media['terrain']['length'], media['terrain']['sha256'], roots)
        if pack is not None:
            (out / 'terrain.pack').symlink_to(pack)
    report = tool('rebuild', trace, '--pulled', out).splitlines()
    timing['total_s'] = round(time.monotonic() - started, 3)
    result.update(media=str(out), origin=report[0].removeprefix('origin '), rebuilt=report[1:], timing=timing)
    if problems:
        result['problems'] = problems
    return result
