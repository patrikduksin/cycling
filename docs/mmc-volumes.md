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
the card only while it is open: it stages at the first such pass, and the
file opens once VANA's boot scans end, so the header can name the media
they found. A pass drains one record into it while a ride records, and up
to 16 otherwise, to keep up with a phone upload; the file syncs every
5 s, before a transfer session, a restart and a terminal power request,
and in the pass before VANA asks to turn off: the file ends with the pass
before that request. The mechanism is `firmware_services::trace::spool`;
the simulator records through it too.

- No STORE volume when the card first opens, or no PSRAM for the spool:
  recording is off for the session, logged under the `trace` target with
  the reason, and nothing else changes.
- After a transfer session, a host write to `FILES` or `TERRAIN`, which
  the trace does not see, ends it with a `HostWrote` record; so does a
  `STORE` volume that a volume command moved. Any other change to the
  store reopens the file, which continues in a new chunk.
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
cycling trace pull [FILE]                     # a ride trace and the media it started on, read only
```

`cycling trace pull` copies a ride trace into `.local/traces/` and rebuilds
the media it started on beside it, as `<name>.origin`, for a replay. It reads
only: ride export up to the ends the trace header records, `files get` for
each file the header lists, and a local terrain pack with the header's
SHA-256. The [ride-trace skill](../.agents/skills/ride-trace/SKILL.md) says
what a rebuild cannot give back.

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
