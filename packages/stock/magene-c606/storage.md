# Storage

## Recovered SD/MMC interface

Static analysis of stock release 1.956 found a four-bit SD/MMC configuration. The
stock mount path is `/sdcard`, host slot 1 is selected, and the slot configuration
is overwritten with these GPIO numbers immediately before the mount call:

| Signal | GPIO |
| --- | ---: |
| CLK | 13 |
| CMD | 14 |
| D0 | 16 |
| D1 | 17 |
| D2 | 18 |
| D3 | 15 |

Card-detect and write-protect are disabled in that configuration. A bounded native
one-bit probe verified slot 1 with CLK, CMD and D0 on GPIO13, GPIO14 and GPIO16,
and identified the fitted device as high-capacity MMC/eMMC. That probe left D1
through D3 in pulled-up input mode; later four-bit read evidence is recorded below.
EXT_CSD reports 3,959,422,976 bytes in 512-byte sectors. The raw CID, CSD, sector
hashes and read traces remain private.

The original optional probe used `CYCLING_SDMMC_PROBE=1`. The bounded raw
read-only backend is available through ordinary `MMC` commands; its implementation
is in [sdmmc.rs](../../os/src/device/sdmmc.rs).

The probe uses slot 1, one-bit width, 400 kHz and 3.3 V signaling. It issues only
the audited identification, selection and read commands. On this unit SD CMD8 and
CMD55 did not respond; after a separate CMD0, MMC CMD1 did. The probe read EXT_CSD,
sector zero twice and the final reported sector. Both sector-zero reads matched,
and all three bounded reads completed on two boots with the same private hashes.
Every transfer stops or resets IDMAC before its aligned internal buffer leaves
scope. It does not mount, format, repair, erase or write the medium.

[#21](https://github.com/patrikduksin/cycling/issues/21) records the probe and safe
stock-selection/restoration evidence. Selection and checksum verification did
not establish visual stock startup or stock-filesystem behavior.

## Read-only layout and clock validation

Harness-enabled base revision `740b36c83e76` identified 7,733,248 sectors and
completed the bounded layout investigation. Sector zero contains a FAT32
superfloppy volume covering the entire reported medium, with eight sectors per
cluster, 964,608 data clusters, and the data region starting at sector 16,384.
The parser read four distinct sectors and reached the root directory terminator.
That establishes a complete root listing for this traversal; child directories,
allocation consistency and stock resource/update behavior were not exhaustively
inspected. Raw directory names, identifiers, sectors and captures remain private.

Repeated reads of sectors 0, 1 and 2,048 returned matching per-sector CRCs at
400 kHz, 4 MHz and 20 MHz. Representative repeated sector-zero device read times
were 10,934, 1,506 and 670 microseconds, respectively. Each terminal response
returned 256 bytes from a bounded sector read. These samples establish successful
reads at those controller clocks, not sustained filesystem or USB throughput.
GNSS and companion observations continued without new errors during the scenario.
The harness verified restoration of the original controller clock and preferences.

SDK revision `405a4a6` also read 64 sectors at each clock while receiving the
owned laptop HRS fixture, with Wi-Fi connected, GNSS/companion acquisition and
one verified screenshot transfer per clock. All 64 per-sector CRCs agreed across
the three passes. Mean full-sector backend times were 11,065.7, 1,637.7 and 817.1
microseconds, corresponding to about 46.3, 312.6 and 626.6 kB/s of backend read
work. These are bounded 64-read bursts; they exclude USB transport overhead and
do not characterize filesystem workloads or long-duration throughput.
The scenario observed 41 BLE notifications, 720 companion reports and 441 valid
GNSS sentences, with no new notification drops, input loss, CRC, UART, DMA or
parser errors. HRS remained decoded and fresh, and cleanup restored the clock,
original BLE selection/runtime echo and preferences. The host fixture was stopped.

The host investigator [mmc.py](../../../scripts/mmc.py) validates each sector as
two CRC-checked chunks, limits distinct reads to 128, and bounds FAT root traversal.
It stores findings only in a new ignored `.local/` directory. It does not recurse
through the vendor tree or issue filesystem writes.

Those investigations issued no MMC writes. Read success and apparently empty
sectors did not establish write ownership. The subsequently authorized partition
plan and guarded bulk backend are described below; settings and rides remain in
their existing explicit `ota_1` reservations.

## Stock resource and update use

A bounded static trace of main update N21 1.956 establishes uses beyond the root
listing. The paths below are constants embedded in that firmware, not filenames
copied from this unit's user data. Addresses are recovered call sites; these
routines were not invoked during the read-only investigation.

| Stock use | Recovered path and operation | Evidence |
| --- | --- | --- |
| Audio configuration | `/sdcard/AUDIO/audio_config.json` opened in `r` mode, sized by seek/tell, read into a buffer, closed, then passed to the JSON parser | Entry `0x4216cf34`; open at `0x4216cf40`, read at `0x4216cf8b`, close at `0x4216cf9a` |
| Audio download state | `/sdcard/AUDIO/downloading.json` opened in `w` mode and written; another routine opens the same path in `r` mode and reads it | Write path `0x4216d6b4`–`0x4216d6df`; read entry `0x4216da4c` |
| Fonts | Six `S/sdcard/FONT/SHS_*` font paths are passed to the same font-loader entry `0x4218de0c` with embedded fallback pointers | Initialization sequence beginning `0x420959a1`; this establishes resource references, not a complete audit of the loader's I/O |
| Update source | `GetUpdateDir` uses `/sdcard/APP` as its directory base; `UpdateInit` opens the selected filename in `rb` mode and stores its file handle | Directory setup `0x421b2d13` and open at `0x421b2d5a`; update-file open `0x421b271d`–`0x421b2728` |

The shared file-open wrapper is `0x4200a604`; its underlying open call receives
the original path and mode at `0x4200a62b`–`0x4200a632`. The corresponding read,
write, seek and close wrappers are `0x4200a984`, `0x4200aa48`, `0x4200af4c` and
`0x4200ac1c`. The audio state writer demonstrates that stock treats this mounted
volume as mutable application storage, rather than merely a read-only resource
image. This observation does not authorize cycling writes.

For the main-MCU update, `ReadProgData` at `0x421b29bc` seeks within the opened
update file to payload offset plus 128, reads a chunk and applies a four-byte
repeating XOR transformation recovered from update metadata. The `MidOTAUpgMS`
path calls it at `0x421b37da`, then passes the resulting buffer and length to the
OTA writer at `0x421b37e3`. Error strings identify the begin/write/end operations
as `esp_ota_begin`, `esp_ota_write` and `esp_ota_end`; their call sites are
`0x421b35ce`, `0x421b37e3` and `0x421b3765`. The destination comes from the OTA
partition selector at `0x421b35c4`, not a sector offset in the source FAT volume.
The success path then calls the boot-partition setter at `0x421b3898`. Thus the
recovered main update flow reads a staged filesystem source and writes an
application flash partition. No update, boot selection or source cleanup was run.

These traces establish specific resource, mutable download-state and update-source
roles. They do not enumerate every path, prove the installed stock version uses
every branch, map all live allocations, or establish any unused namespace. In
particular, absence of a directory or an apparently empty FAT cluster does not
make it application-owned. At the time of this investigation, the whole-medium
FAT geometry left no established writable bulk region for cycling.

## Stock partition selection and automatic formatting

A static trace of N21 release 1.956 on 2026-09-14 found explicit MBR support in
the stock FatFs mount path. This is evidence from the recovered release image;
the trace alone does not establish a successful boot with a partitioned MMC.

`MidVFSMount` at `0x4200b0e8` calls `esp_vfs_fat_sdmmc_mount` at
`0x42214da4`. The mount configuration has 17 open files and a 16,384-byte
allocation unit. Startup passes argument 1 at `0x42009d8c`; the wrapper uses
that argument to enable `format_if_mount_failed` at `0x4200b141`.

The FatFs logical-to-physical mapping at `0x3c682b2f` maps logical drive zero
to physical drive zero with partition selector zero. In `mount_volume` at
`0x4221554c`, that selector means automatic discovery. The code checks sector
zero first with `check_fs` at `0x422151b4`. If it is an MBR rather than a FAT
boot sector, the code at `0x42215605` reads the four primary partition start
LBAs from byte offsets 454, 470, 486 and 502. It tries nonzero starts in entry
order and accepts the first recognized FAT volume. The recovered scan uses
the boot-sector contents, not the MBR partition type, to recognize a filesystem.

A conventional MBR with stock FAT32 as the first primary partition, aligned at
LBA 2,048, is therefore a supported candidate for hardware testing. The stock
filesystem must remain valid and fit entirely inside its partition. A second
FAT filesystem is not protected by its position or a different MBR type: if the
first volume stops being recognized, automatic discovery can mount the second.

There is also a whole-medium recovery hazard. The mount helper at `0x42214bec`
handles FatFs errors 13 and 2 by checking the format flag, allocating a work
buffer, and calling `f_fdisk` at `0x42217bd4` with a partition size list of
`{100, 0, 0, 0}`. It then calls `f_mkfs` at `0x422174fc` and retries the mount.
The corresponding call sites are `0x42214c43`, `0x42214c78` and `0x42214c8d`.
This fallback can replace an experimental partition table with one partition
using the medium, destroying the custom allocation boundary. Partitioning alone
does not isolate custom data from stock's format-on-failure behavior.

Before a stock boot after conversion, validate the first partition's filesystem
and preserve a verified full MMC image. Keep any second partition disposable
through compatibility tests, inspect the MBR and both partition boundaries after
stock boot, and test custom read/write behavior only against an explicitly owned
range. This investigation did not run the formatting branch or establish that
all stock code paths honor the mounted volume boundary.

## Partition layout and bulk backend

On 2026-09-14 the owner explicitly authorized repartitioning, vendor-filesystem
writes, data erasure and device experiments, provided backups and a recovery path
are preserved. This overrides the earlier filesystem-write restriction for this
work. The bootloader, internal flash partition table and eFuses remain protected.
The following partition layout has been written and checked as described below.
The owner confirmed normal stock UI startup after conversion. Subsequent SDK
checks found the MBR and ownership marker unchanged; this does not establish
every stock resource, update or format-recovery path.

| Region | Start sector | Sectors | Bytes |
| --- | ---: | ---: | ---: |
| MBR and alignment reservation | 0 | 2,048 | 1,048,576 |
| Stock FAT32, primary partition 1, type `0x0C` | 2,048 | 1,953,792 | 1,000,341,504 |
| Custom reservation, primary partition 2, type `0xDA` | 1,955,840 | 5,777,408 | 2,958,032,896 |

The custom partition begins with one 512-byte ownership marker. Application data
starts at sector 1,955,841 and excludes that marker. The schema and parser in
[bulk.rs](../../os/src/bulk.rs) bind `CYCLING-BULK`, version 1, sector size, both
partition extents and total capacity to the complete MBR's CRC32, with a separate
marker CRC32. The backend checks ownership before writes and rejects a changed
MBR binding until reboot. Writes with the eMMC cache enabled are unsupported.
The marker is a guard for cycling software; it cannot prevent stock's automatic
repartitioning and formatting described above.

Normal raw `MMC` access remains read-only. `MMC OWNED STATUS` and `MMC OWNED READ`
address the marked application region. Harness-only `MMC OWNED TEST` accepts a
relative sector, its expected current CRC32 and a fill byte, then writes one
512-byte pattern and verifies readback. Use terminal `HELP` for operative syntax.
There is no ride migration in this change; long recordings on bulk storage remain
separate work in [#105](https://github.com/patrikduksin/cycling/issues/105).

## Backup, offline planning and maintenance

`CYCLING_BULK_MAINTENANCE=1` selects a dedicated image whose binary USB protocol
replaces the normal console. Install it through the existing protected
`mise run flash` workflow. The host [mmc_maintenance.py](../../../scripts/mmc_maintenance.py)
uses the shared USB lock and opens without reset. `mise run mmc-maintenance --help`
and subcommand help own the current CLI syntax.

`backup IMAGE` creates a new full MMC image and manifest under ignored `.local/`.
It marks the backup verified only after a second full device read matches every
byte. Explicit `backup IMAGE --resume` can continue an unverified whole-sector
prefix after an interrupted read, including sparse zero sectors. It never
overwrites a verified baseline and still requires full independent verification.
Keep a separately verified copy outside the repository before conversion.
The planner and executor also accept explicit `--single-read-copy COPY` for an
owner-authorized exception: one complete capture plus a separate regular file
with a different inode, identical size and matching SHA256. That copy may be
outside the repository and is read-only input. The exception leaves the backup
manifest's `verified` flag false; matching copies do not establish independent
device readback. Without this flag, the independent-read requirement remains.

`verify` compares an existing full image against the medium; `read` captures a
specified range. `write` requires the exact source SHA256 and a verified full
backup manifest, or the explicit single-read-copy exception, before arming its range.
Each sector is read back; timeout or
disconnect stops the invocation without reconnecting or replaying the mutation.
`recover` attempts controller recovery, not data restoration.

Explicit `wide` selects four-bit SDR in maintenance mode. It first selects the
card-advertised `POWER_CLASS` for the existing 3.3 V rail and clock range, then
changes `BUS_WIDTH`. Both settings are volatile; it changes neither the voltage
rail nor persistent boot configuration. `INFO` and `READ` never enable this mode
automatically. `recover` uses CMD0 and restores one-bit operation, so widening
after recovery requires another explicit `wide` command. The operation disarms
writes, and a failed switch blocks media access until recovery.

The offline [mmc_partition.py](../../../scripts/mmc_partition.py) takes that backup
manifest and a prepared stock FAT32 image. It validates filesystem geometry and
produces hashed changed-sector extents in a new private plan directory. Apply the
stock extents first, the marker next, and the MBR last. It leaves all other sectors
untouched and performs no device I/O. Review the plan and its CLI help before use.

The [partition executor](../../../scripts/mmc_partition_apply.py) defaults to
offline validation with `--plan PLAN`. It checks the fixed C606 layout, authorized
baseline, target hashes and every extent, including gaps between changed sectors.
`--execute --journal NEW_PATH` enables one attempt and requires a new private,
fsynced journal. Default `--verification full` verifies the entire first partition
after writing its extents. Explicit `--verification written` instead retains each
sector's write/readback check and samples boot sectors, FSInfo, FAT boundaries,
root-directory boundaries and the partition's last sector. Both modes check
untouched boundaries before writing and verifying the marker, then committing
and verifying the MBR last. The journal records the selected verification method;
sampled verification does not claim a full partition readback hash.
No writes or stock boots may intervene between the captured baseline and execution;
the executor's live sentinel reads do not replace that provenance. A failure stops
without retry, automatic rollback or stock boot. Use its `--help` for current syntax.

A full MMC restoration uses the saved MMC image through maintenance mode. The
`write` command also accepts the explicit `--single-read-copy` policy when the
saved baseline has one complete device capture and a distinct matching copy.
The internal 16 MiB `flash.bin` backup cannot restore this separate 4 GB medium.
For this session the owner requested an SDK-enabled image for an immediate first
ride, overriding the usual return to harness-enabled base. SDK installation and
the resulting recording state are recorded below. Existing ride recording still
uses its internal flash reservation, not the new bulk partition.

## Partition conversion and maintenance observations

On 2026-09-14 a complete 3,959,422,976-byte MMC capture and a distinct matching
copy were retained. The owner explicitly stopped the second full read to proceed
faster. The backup remains `complete=true, verified=false`; its separate copy
matches its SHA256, but independent full device-read verification was not completed.

The final stock FAT32 image contains 49 files totaling 19,670,744 bytes, retaining
fonts and small settings. At the owner's request, map, log, FIT and staged-update
files were omitted while their directories were retained. The image started from
the baseline bytes at the new partition's location before formatting and copying
the retained files, so unallocated bytes were not cleared. `fsck` passed with
4,873 of 243,742 clusters allocated. This is filesystem preparation, not secure
erasure of the omitted files.

The conversion completed using 87 extents covering 38,650 sectors, or 19,788,800
bytes. Every written sector received device readback verification. Stock metadata
and boundary samples matched, untouched boundaries matched the baseline, and the
ownership marker and final MBR matched exactly. This used the explicit single-read
copy and written-sector verification policies, not a full readback of the new
stock partition. The owner then confirmed that stock booted normally on the
physical screen. Afterward, SDK readback found the MBR and ownership marker still
matching the completed plan exactly.

With `CYCLING_SDK=1` and `CYCLING_HARNESS=1` installed, normal owned-region tests
wrote a `0xA5` pattern to relative sector 0 at 400 kHz and relative sector
5,777,406 at 20 MHz. Both reads matched, and both sectors were restored and read
back successfully. An incorrect expected CRC refused the write without changing
data; out-of-range reads and writes returned `INVALID`. The MBR and marker stayed
unchanged. USB-inclusive write commands took 96.96 ms at 400 kHz and 24.90 ms at
20 MHz, including ownership validation and reads around the write. These are
command timings, not raw media throughput.

The internal ride journal initially reported `needs_init` with 665 occupied slots.
Under the owner's explicit permission to discard that data, `RIDE INIT` completed,
then `RIDE START` completed. Four samples committed with zero reported drops.
The device was left running SDK firmware with the live ride recording, ready to
continue after USB disconnection. This ride uses internal flash; recording onto
the new bulk partition still requires the separate work in #105.

A preliminary single-sector pattern write in one-bit mode at 20 MHz took 3.192 ms
including its USB acknowledgment. Readback after recovery matched the pattern;
the original sector was restored and matched again after recovery. This is one
bounded write/restoration observation, not a sustained-write throughput result.

On 2026-09-14 explicit four-bit mode read sectors 0, 1,310, 16,384 and 7,733,241
with results matching earlier one-bit reads. Another 24 randomly selected
locations in the captured prefix matched across two recovery cycles. These
observations establish successful reads using D1 through D3 and successful
re-entry into four-bit mode after recovery; they do not establish long-duration
reliability. The write observations above used one-bit mode.

A 1 MiB nonuniform transfer over the binary USB protocol took 3.30 seconds.
During a mostly free portion of the backup, observed progress was about 2.3 MB/s
in four-bit mode versus about 1.28 MB/s in one-bit mode. These are workload samples,
not a controlled sustained-throughput benchmark. Uniform-sector compression
reduces USB traffic during the mostly free portions, so those rates are effective
image progress rather than raw USB payload throughput.

## Owned application storage

Settings and rides use explicit reservations inside slot B. The operative geometry
and protected application limit are owned by [device.py](../../../scripts/device.py)
and [device storage](../../os/src/device/storage.rs), with host boundary tests.
The stock partition table describes the original slots, not the smaller application
extent after those reservations.

Routine safe flashes preserve both journals. A tool that writes all of `ota_1`
can erase them. Settings use alternating committed sectors with CRC validation;
ride compatibility and recovery belong to the SDK. See
[ride recording](../../../docs/ride-recording.md) for export and explicit reclaim.
Unknown settings content refuses writes rather than initializing over data.

The original settings bring-up on 2026-09-11 loaded the same committed record
after a second protected flash. Later save/restart/restoration and preserved ride
prefix evidence is in [PR #71](https://github.com/patrikduksin/cycling/pull/71#issuecomment-5647388096).
