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
cannot use (newer version, unknown kind code, or extents beyond the region)
is `foreign` and is never overwritten.

A volume is identified by its name. Each table entry also has a kind code
byte, which firmware from before `STORE` checks: it accepts only 1, 2 and 3
and reports any other code as `foreign`. Every table therefore writes
`RIDES` as 1, `TERRAIN` as 2, and `FILES` and `STORE` as 3. A store is
identified by the magic in its own header, not by this code.

`STORE` is the last volume. New data areas are directories in it, not new
volumes.

## Creating the table, and regions from before volumes

Only an explicit initialize writes the table: `RIDE BULK INIT`, or
`VOLUME INIT` / `cycling volumes init` on builds without VANA. It writes
two sectors, and then `RIDE BULK INIT` formats the log inside `RIDES`.

Before volumes, the ride log owned the whole region, starting at relative
sector 0. Such a region is reported as `layout=legacy`, and `RIDE BULK STATUS`
shows `state=needs_init`. Nothing is written until an initialize, which makes
the old records unreachable. If both table copies are damaged but `RIDES`
still holds a log, the layout is `lost` (the rides are corrupt, not blank).
`VOLUME INIT` then rewrites the default table, which restores access to the log.

## Adding STORE to a table from before it

Tables written before `STORE` existed hold `RIDES`, `FILES` and `TERRAIN`
and leave about 260 MiB unallocated. Only the owner adds `STORE`, with
`VOLUME ADD STORE` or `cycling volumes add-store`:

1. The command reads the table. It writes nothing when there is no table
   (`blank`, `legacy` or `lost`: `UNINITIALIZED`) or the table is `foreign`.
2. It adds one extent, `STORE`, from the 1 MiB boundary after the last volume
   to the end of the region, and increments the generation. No other extent
   moves or changes.
3. It reads the two store header sectors of `STORE` (volume sectors 0 and
   32). If neither one holds the store magic, the volume is blank. If one
   holds the magic but no header is valid, the reply is `CORRUPT`. If a
   header is from another store version or geometry, the reply is `FOREIGN`. In both
   cases the command writes nothing, not even the table.
4. It writes the new table first to the copy that does not hold the current
   table, then to the other copy. A power cut leaves the old table or the
   new one.
5. It formats an empty store in a blank volume. A store that exists is kept.

A blank volume is defined by the magic, not by zero bytes. Sectors that
were never written can hold stale bytes of an old filesystem, as the
simulator and the test media model them, or read as zeros or 0xFF when
erased. None of these hold the magic. The format writes its header first without the magic and then
complete. A power cut tears a sector into new and old bytes, so a torn
header is either complete or has no magic. A format that a cut stopped is
therefore still blank, never `CORRUPT`.

The command is idempotent. Run it again after a power cut: it writes the new
table only to a copy that does not hold it yet, then formats the store if
the volume is still blank. A table that already has `STORE`, such as one from
`VOLUME INIT`, only gets its store formatted. The property test in
`packages/services/src/volume/tests.rs` cuts power at every write of the
migration.

### Rolling back after the migration

Firmware from before `STORE` still accepts a migrated or fresh table,
because `STORE` uses kind code 3. That firmware finds `RIDES`, `FILES` and
`TERRAIN` by name and does not use `STORE`. It never writes a table over an
existing one, so `STORE` and its files remain for the next newer build.
`cycling rollback` keeps rides, files and terrain available. The test
`firmware_from_before_store_still_opens_its_volumes_in_a_table_with_store`
decodes both tables with a copy of that firmware's decoder.

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

`STORE FORMAT CONFIRM` (or `cycling store format --confirm`) is the owner's
way out of a `CORRUPT` store, which `VOLUME ADD STORE` never formats over.
It erases every file and starts an empty store with a new store id. It never writes over a `FOREIGN` store. It is a line
command, so it never runs during a transfer session, which keeps the store
open; nothing else holds `STORE` open yet.

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
cycling volumes add-store                     # add STORE to a table from before it
cycling terrain-write pack.bin --tag CHILE-V1 --verify   # resumes a matching writing pack
cycling terrain-verify pack.bin
cycling files list | put NAME PATH | get NAME PATH | delete NAME | format --confirm
cycling store list [traces/] | get NAME PATH | delete NAME | format --confirm
```

`cycling store get` pulls a file record by record. The session keeps the
file open between requests, so a pull reads about one sector per 486 bytes,
and at the end the rest of the file's last chunk (at most 512 sectors). If
the device lost records of the file, `get` stops at the first one: `PATH`
keeps the records before it, and the command names the lost record and
exits with status 1.
A delete changes the store while the app is paused. After the session the
app mounts the store again and compares its generation with the one it held;
if they differ, it reopens its files.

Line commands: `VOLUME [STATUS|INIT|ADD STORE|TRANSFER]`, `TERRAIN [STATUS]`,
`FILES [STATUS|FORMAT CONFIRM|LIST slot|STAT name|DELETE name]` and
`STORE [STATUS|FORMAT CONFIRM|LIST index]`. The simulator supports the line commands. Its
`Sim::install_terrain`, `Sim::put_file`, `Sim::file`, `Sim::format_store` and
`Sim::store_file` stand in for the host transfer.
