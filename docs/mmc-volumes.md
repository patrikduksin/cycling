# MMC volumes

The owned MMC region (about 2.75 GiB after the stock FAT32 partition) is split
into named volumes by a small table. Each volume is a block device of its own,
so code using one volume cannot reach another one or the table.

| Volume | Default size | Owner | Holds |
| --- | --- | --- | --- |
| `RIDES` | 1 GiB, about 2,330 h | VANA (`apps/vana/src/ride/bulk.rs`) | The sector log of bulk rides ([ride recording](ride-recording.md)) |
| `FILES` | 256 MiB | `firmware_services::files` | Up to 128 named files of at most 1 MiB: workouts, companion settings |
| `TERRAIN` | 1.25 GiB | `firmware_services::pack` | One opaque pack written by the host and read by the firmware |
| unallocated | about 260 MiB | | Room for later volumes |

The layout code is `firmware_services::volume`. Relative sectors 0 and 1 hold
two CRC-sealed copies of the table, and volumes start on 1 MiB boundaries.
Readers use the valid copy with the newest generation. A table this firmware
cannot use (newer version, unknown kind, or extents beyond the region) is
`foreign` and is never overwritten.

## Creating the table, and regions from before volumes

Only an explicit initialize writes the table: `RIDE BULK INIT`, or
`VOLUME INIT` / `mise run volumes -- init` on builds without VANA. It writes
two sectors, and then `RIDE BULK INIT` formats the log inside `RIDES`.

Before volumes, the ride log owned the whole region, starting at relative
sector 0. Such a region is reported as `layout=legacy`, and `RIDE BULK STATUS`
shows `state=needs_init`. Nothing is written until an initialize, which makes
the old records unreachable. If both table copies are damaged but `RIDES`
still holds a log, the layout is `lost` (the rides are corrupt, not blank).
`VOLUME INIT` then rewrites the default table, which restores access to the log.

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
mise run volumes -- inspect                       # table, terrain state, file store
mise run terrain-write -- pack.bin --tag CHILE-V1 --verify   # resumes a matching writing pack
mise run terrain-verify -- pack.bin
mise run files -- list | put NAME PATH | get NAME PATH | delete NAME | format --confirm
```

Line commands: `VOLUME [STATUS|INIT|TRANSFER]`, `TERRAIN [STATUS]` and
`FILES [STATUS|FORMAT CONFIRM|LIST slot|STAT name|DELETE name]`. The simulator
supports the line commands. Its `Sim::install_terrain`, `Sim::put_file` and
`Sim::file` stand in for the host transfer.
