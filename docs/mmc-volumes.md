# MMC volumes

The owned MMC region (about 2.75 GiB after the stock FAT32 partition) is split
into named volumes by a small table. Each volume is a block device of its own,
so code using one volume cannot reach another one or the table.

| Volume | Default size | Owner | Holds |
| --- | --- | --- | --- |
| `RIDES` | 1 GiB, about 2,330 h | VANA (`apps/vana/src/ride/bulk.rs`) | The sector log of bulk rides ([ride recording](ride-recording.md)) |
| `FILES` | 256 MiB | `firmware_services::files` | Up to 128 named files of at most 1 MiB: workouts, workout records, ride facts, bests, the course, companion settings |
| `TERRAIN` | 1.25 GiB | `firmware_services::pack` | One opaque pack written by the host and read by the firmware |
| `STORE` | the rest, about 260 MiB | `firmware_services::store` | Growable append-only files in directories, such as ride traces in `traces/` |

The layout code is `firmware_services::volume`. Relative sectors 0 and 1 hold
two CRC-sealed copies of the table, and volumes start on 1 MiB boundaries.
Readers use the valid copy with the newest generation. A table this firmware
cannot use (another version, or extents beyond the region) is `foreign` and
is never overwritten. A volume is identified by its name; what it holds is
identified by the magic in its own header.

`STORE` is the last volume. New data areas are directories in it, not new
volumes.

## Creating the table

Only an explicit initialize writes the table: `RIDE BULK INIT`, or
`VOLUME INIT` / `cycling volumes init`. It writes
two sectors, and then `RIDE BULK INIT` formats the log inside `RIDES`.
`STORE FORMAT CONFIRM` formats an empty store inside `STORE`.

If both table copies are damaged but `RIDES` still holds a log, the layout is
`lost` (the rides are corrupt, not blank). `VOLUME INIT` then rewrites the
default table, which restores access to the log.

## FORMAT

Every persisted format has one version byte, and data with another one is
`foreign`: no reader accepts it and no format, create or initialize writes
over it ([AGENTS.md](../AGENTS.md) states the rule). Here
that covers the settings journal and the records in it, the ride slots, the
volume table, the sector log, FILES, STORE, the pack, and each table VANA
writes into FILES.

A boot that finds a foreign store asks `FORMAT STORAGE?` once, from the brand,
Home or setup, never over a ride:

- ACT (trash cue) wipes the stores that read foreign when the question was asked, and
  nothing else: a foreign volume table takes the table, the terrain pack and
  the `RIDES`, `FILES` and `STORE` volumes; a foreign ride log, `FILES` (or a
  record in it) or `STORE` takes that volume; a foreign internal ride journal
  takes the journal; foreign settings take the settings (shell settings,
  preferences, the Wi-Fi password). The question's last line names what goes:
  `RIDES AND SETTINGS GO` when the rides and the settings both do, the one
  store when only one does (`WHOLE CARD GOES`, `CARD RIDES GO`,
  `CARD FILES GO`, `TRACES GO`, `DEVICE RIDE LOG GOES`, `SETTINGS GO`), and
  otherwise the gravest of them and more (`RIDES AND MORE GO`,
  `SETTINGS AND MORE GO`, `FILES AND MORE GO`, `RIDE LOG AND MORE GO`). The
  screen then says `CHECKING STORAGE` and the device restarts, into first-run
  setup only when the settings went. Only a press that begins after the
  question is drawn counts, so a button held through boot does nothing.
- BACK keeps everything. Stores of the current version keep working, the
  ready page says `STORAGE NOT FORMATTED` / `OFF AND ON TO FORMAT` where a
  ride cannot record, settings say `NOT SAVED: DATA KEPT`, BESTS says
  `BESTS NOT FORMATTED` and offers no CLEAR BESTS, `RIDE INIT` and
  `RIDE CLEAR CONFIRM` answer `FOREIGN` over a foreign ride log, and the next
  boot asks again.

`FORMAT CONFIRM` on the terminal, or `cycling format --confirm`, wipes every
store, foreign or not; with no card, only the internal journal and the
settings. `FORMAT`, or `cycling format`, reports
each store as `current`, `blank`, `foreign` or `unusable`, in the order the
wipe takes them: `table`, `rides`, `files`, `records` (the facts, deleted
rides, bests and workout outcomes VANA writes into FILES), `traces` (STORE),
`internal` and `settings`. Each record's owner (`kept/facts/record.rs`,
`kept/deleted.rs`, `kept/bests.rs`, `workout/history.rs`) decides what its file
holds, and a file that names the owner but carries another version byte is
foreign.

What the wipe leaves alone:

- The terrain pack, unless the volume table itself was foreign (the default
  table is written and the pack goes with it) or the pack's own header is
  foreign. A foreign pack alone raises no offer; the map draws without roads.
- Files the host wrote into FILES are erased with it, but a foreign course,
  turns or workout file alone raises no offer: load it again.
- The crash record, the stock backup and the firmware slots.

Damage is not foreign. A lost table, a corrupt log, a damaged record or a
failing card reads `unusable` or `current`, shows `STORAGE PROBLEM` where a
ride cannot record and raises no offer; `FORMAT CONFIRM` still rewrites it.
The internal ride journal is the exception: it cannot tell a slot of another
version from its own torn appends, so data it cannot read is `foreign`, raises
the offer, and only FORMAT erases it.

From its first write until the restart, the wipe owns storage: no ride starts
(`STORAGE NOT READY`) and storage commands answer `BUSY`; `RIDE STATUS`
still answers. The trace file ends only when the wipe takes `STORE`; the
trace writes nothing else, so it goes on through any other wipe. Once it erases the settings, nothing
saves them until the restart. A wipe of the internal journal alone ends
without a restart: the journal starts empty and the first page shows.

Before its first write, the wipe records which stores it takes in the
intent journal, and it settles that journal only after its last stage. A power cut during the wipe never touches
a store it was not for, and the next boot finishes it. Until a boot has read
the intent, no ride starts, no question is asked and storage commands answer
`BUSY`. With a wipe due, the boot programs the commit word of every pending
record that reads whole again, in place, and reads each back (a recommit).
It runs the wipe's stages only once every recommit returned, once storage
has been read, and restarts, or for the internal journal alone, shows the
first page. A boot that cannot read or recommit the intent tries again each
pass, as one that cannot read it does. A card that is missing or failing
when the wipe resumes is left out of it, as `FORMAT CONFIRM` leaves it out,
so the boot-flash stores still go. The screen goes from the brand
to `CHECKING STORAGE`, unless the wipe is done before the brand ends, and to
first-run setup afterwards only when the settings went. Every stage converges
from any cut state of itself. A cut before the intent is recorded leaves
everything as it was, and the offer comes back. `RIDE CLEAR CONFIRM` on the
internal journal runs this wipe for the journal alone, so a Clear cut by
power is finished at the next boot and never reads as another version's.

A wipe of the internal journal also forgets what `FILES` records of its rides
(workout outcomes, facts and deleted rides), since its next rides number from
1 again, unless `FILES` goes in the same wipe. A card that is missing,
foreign or unusable during the wipe keeps those records, and a new ride of
the same number, start and length may show as deleted. A card that refuses
a read or write while the records go fails the wipe, as any card stage does,
and the records it kept do the same.

A wipe whose write fails, as a dead card's does, settles the intent, stops
and shows the ready page with `STORAGE NOT FORMATTED`, and a Clear waiting
on it completes with `result=FAILED`; the next boot offers
it again for whatever still reads foreign. Only a wipe that ended this boot
settles: one whose intent could not be recorded, or a boot that cannot read
the intent, leaves it for the next boot. A boot that still cannot read the
intent once storage has been read shows the same ready page, and a power
cycle reads it again.

The intent journal is two sectors of internal flash at `0x1C000`, below the
crash record, outside every store. Its one record is a version byte and the
set of stores still to wipe, empty when none is due. A wipe is due when
either sector holds a committed record that names a store: a commit word cut
by power may read whole on one boot and torn on the next, so the newest
record alone does not decide.

Stages run only behind a commit that completed: the begin's own, or a
recommit. NOR programming only clears bits, so programming the same commit
word over a marginal one completes it and changes nothing else, and a
completed commit never reads torn later. A record that a boot reads torn
therefore had no stage behind it, and dropping it drops a wipe that never
began. A begin never saves beside a pending record, since a save erases a
sector that may hold its only copy: it recommits the pending records and
runs them when they cover what it was asked for, and otherwise records
nothing and fails. A settle leaves both sectors blank or idle.

The intent journal is the deliberate exception to the rule on foreign data: bytes it cannot read, the
stock image's or another version's, read as no wipe due, and every boot
writes over them. `FORMAT CONFIRM` never wipes it, and flash and rollback
never write it; a full stock restore (`cycling stock --confirm-restore`)
writes the backup's `0xff` over it.

The code is `apps/vana/src/app/storage.rs` (what each store holds) and
`apps/vana/src/app/format.rs` (the offer and the wipe), and
`apps/vana/src/app/intent.rs` (the intent journal). The only functions that
write over a foreign store are `erase` in the sector log, files, store and
pack, `volume::replace`, `ride::bulk::wipe`, `ride::journal::wipe_sector` and
the settings journal's `erase`.

## Store

`STORE` holds growable files that are written by appending. A name is a path
such as `traces/0001`, and a directory is a name prefix such as `traces/`. Two
copy-on-write metadata copies hold the directory (64 files at most) and a
table of 256 KiB data chunks. Each data sector is a self-checking record, so
an append writes no metadata and a power cut loses at most the bytes appended
since the last sync. When the store is full, an appender given a directory
deletes the oldest closed file in it (rollover). Otherwise only the host
deletes a file, with `cycling store delete`; there is no line command for it.
Every operation does a bounded number of sector operations, whatever the
volume size. The module documentation in `packages/services/src/store.rs`
lists them, and the power-cut property test in `store/tests.rs` cuts power at
every write.

While a chunk belongs to a file, each of its record sectors is written once.
A file that is opened again, after a power cut or a transfer session, reads
its last chunk back to the last record that reads back, and continues in a
new chunk. The chunk table records how many records the old chunk holds,
and the rest of it stays unused. The file never continues after the records
it finds: a record that the medium lost could hide records after it, which a
new record would then overwrite. Only a last chunk that holds no record is
written from its start. An appender that gets an error writes nothing more
until the file is opened again.

A reader reports a lost record as a hole, with its record number, and
continues after it. Records lost at the end of a file's last chunk, or
before a reopen read that chunk back, read as the end of that chunk.

`STORE FORMAT CONFIRM` (or `cycling store format --confirm`) formats a new
store and is the owner's way out of a `CORRUPT` one. It erases every file and starts an empty store with a new store id. It never writes over a `FOREIGN` store. It is a line
command, so it never runs during a transfer session, which keeps the store
open, and the C606 refuses it with `BUSY` while a ride records or while
the session's trace file is open.

### Ride traces

The C606 records each power-on session into
`traces/<sequence>-<boot id>.trace`, the sequence one past the highest in
`traces/`. Blocks wait in a PSRAM spool from the first pass. The power
owner closes the card from the moment it accepts a power request until
the device is ready again, and in charging standby, so the trace touches
the card only while it is open: it stages at the first such pass, before
VANA reads the card, and the file opens in that pass. The header is synced
as the file is made, so a file cut off at once still decodes. A pass drains one record into it while a ride records, and up
to 16 otherwise, to keep up with a phone upload; the file syncs every
5 s, before a transfer session, a restart and a terminal power request,
and in the pass before VANA asks to turn off: the file ends with the pass
before that request. The mechanism is `firmware_services::trace::spool`;
the simulator records through it too.

- No STORE volume when the card first opens, or no PSRAM for the spool:
  recording is off for the session, logged under the `trace` target with
  the reason, and nothing else changes.
- After a transfer session, a host write to `FILES` or `TERRAIN`, which
  the trace does not see, makes the trace forget the card (a `Forget`
  record): the next read of each sector records its bytes again. A `STORE`
  volume that a host or a volume command moved ends the trace. Any other
  change to the store reopens the file, which continues in a new chunk.
- A burst the spool cannot hold is lost from the trace, which names the
  loss in its next block. A phone upload of the largest body the phone
  server takes (4 MiB) peaks the spool at about two blocks.
- Secrets are redacted as they are recorded; see
  `firmware_services::trace::redact` for what a replay does with them.

`STORE STATUS` adds `trace=waiting|off|staging|recording|ended`, the
file and the bytes written, `why` (words joined by `_`) when it is off or
ended, and the spool's use, peak and refused blocks.

## Files

Each change to `FILES` is copy-on-write and commits with a single sector
write. A file's bytes go to free sectors first. The new directory is then
written to the copy that is not current, entry sectors first and the header
last. After a power cut, the store holds exactly the files of the last
committed directory. The property test in `packages/services/src/files/tests.rs`
cuts power at every kind of write. A writer whose directory changed since it
began (for example because the host wrote meanwhile) fails with `Stale` and
commits nothing.

## Terrain pack

The pack header records the host's 16-byte format tag, SHA-256, length, CRC32
and state (`writing` or `ready`). `begin` makes a `writing` header current
before any data changes, so no reader sees a ready pack over bytes that are
being replaced. Data sectors are written in order while the device keeps a
running CRC. `commit` marks the pack ready only when every sector was written,
or read back after a resume, and the CRC matches. Firmware reads it with
`pack::open` and `Pack::read`.

## Host tools

Bulk data moves through a binary transfer session that `VOLUME TRANSFER`
enters on the ordinary terminal. The frames are described in
`packages/console/src/transfer.rs`. The device pauses the app during the
session and refuses to start one while a ride is recording.

```sh
cycling volumes inspect                       # table, terrain state, file store
cycling terrain-write pack.bin --tag CHILE-V1 --verify   # resumes a matching writing pack
cycling terrain-verify pack.bin
cycling files list | put NAME PATH | get NAME PATH | delete NAME | format --confirm
cycling store list [traces/] | get NAME PATH | delete NAME | format --confirm
cycling format [--confirm]                    # every store's health; --confirm wipes them all
cycling trace pull [FILE]                     # a ride trace, read only
```

`cycling trace pull` copies a ride trace into `.local/traces/`. The trace is
a world log: it holds every byte of media VANA read, so a replay needs
nothing else from the device. The pull only reads. It prints its result as
one JSON document, then on stderr the moments the rider marked by holding
MOVE (`marks:` with one line each, or `marks: none`). With `--json` it prints
only the document, and a failure it reports, bad arguments included, as
`{"error": ...}` with exit status 2.

`cycling store get` pulls a file record by record. The session keeps the
file open between requests, so a pull reads about one sector per 486 bytes,
and at the end the rest of the file's last chunk (at most 512 sectors). If
the device lost records of the file, `get` skips them: `PATH` keeps every
record that reads back, and the command names each hole, with its byte
offset in `PATH`, and exits with status 1. If a sector read fails, `get`
stops there: `PATH` keeps every record before it, the result marks the pull
`incomplete`, and the command exits with status 1. A failure before the
first record leaves `PATH` as it was.
A delete changes the store while the app is paused. After the session the
app mounts the store again and compares its generation with the one it held;
if they differ, it reopens its files.

Line commands: `VOLUME [STATUS|INIT|TRANSFER]`, `TERRAIN [STATUS]`,
`FILES [STATUS|FORMAT CONFIRM|LIST slot|STAT name|DELETE name]` and
`STORE [STATUS|FORMAT CONFIRM|LIST index]`. The simulator supports the line commands. Its
`Sim::install_terrain`, `Sim::put_file`, `Sim::file`, `Sim::format_store` and
`Sim::store_file` stand in for the host transfer.
