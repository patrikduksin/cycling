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
cannot use (newer version, unknown kind, or extents beyond the region) is
`foreign` and is never overwritten.

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
3. It writes the new table to copy 1, then to copy 0. A power cut leaves the
   old table or the new one.
4. It formats an empty store in `STORE` if the volume holds none. A store
   that exists is kept.

The command is idempotent. Run it again after a power cut: it writes the new
table only to a copy that does not hold it yet, then formats the store if
needed. A table that already has `STORE`, such as one from `VOLUME INIT`,
only gets its store formatted. Firmware from before `STORE` reports a
migrated table as `foreign` and cannot open any volume until a newer build
runs again. It never overwrites a foreign table, so no data is lost.

## Store

`STORE` holds growable files that are written by appending. A name is a
path such as `traces/0001`, and a directory is a name prefix such as
`traces/`. Two copy-on-write metadata copies hold the directory (64 files)
and a table of 256 KiB data chunks. Each data sector is a self-checking
record, so an append writes no metadata and a power cut loses at most the
bytes appended since the last sync. When the store is full, an appender
given a directory deletes the oldest closed file in it (rollover). Every
operation does a bounded number of sector operations, whatever the volume
size. The module documentation in `packages/services/src/store.rs` lists
them, and the power-cut property test in `store/tests.rs` cuts power at
every write.

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
cycling store list [traces/] | get NAME PATH | delete NAME
```

`cycling store get` pulls a file record by record. The session keeps the
file open between requests, so a pull reads about one sector per 486 bytes.
A delete changes the store while the app is paused. After the session the
app mounts the store again and compares its generation with the one it held;
if they differ, it reopens its files.

Line commands: `VOLUME [STATUS|INIT|ADD STORE|TRANSFER]`, `TERRAIN [STATUS]`,
`FILES [STATUS|FORMAT CONFIRM|LIST slot|STAT name|DELETE name]` and
`STORE [STATUS|LIST index]`. The simulator supports the line commands. Its
`Sim::install_terrain`, `Sim::put_file` and `Sim::file` stand in for the host
transfer.
