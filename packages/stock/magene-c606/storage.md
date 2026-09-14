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
one-bit probe has now verified slot 1 with CLK, CMD and D0 on GPIO13, GPIO14 and
GPIO16, and identified the fitted device as high-capacity MMC/eMMC. D1 through D3
remain stock-derived candidates because the probe leaves them in pulled-up input
mode. EXT_CSD reports 3,959,422,976 bytes in 512-byte sectors. The raw CID, CSD, sector hashes and read
traces remain private.

The original optional probe used `CYCLING_SDMMC_PROBE=1`. The current bounded
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

Read success does not grant write ownership. Empty sectors or gaps would not prove
that stock ignores them. Settings and rides remain in their existing explicit
`ota_1` reservations. A future bulk backend must extend the bounded stock-use and allocation audit
and obtain an
explicitly owned namespace or region with bounded and recoverable writes. Until
then, no cycling code writes this MMC device.

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
make it application-owned. The whole-medium FAT geometry and these stock uses
leave no established writable bulk backend for cycling.

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
