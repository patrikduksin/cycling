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

The host investigator [mmc.py](../../../scripts/mmc.py) validates each sector as
two CRC-checked chunks, limits distinct reads to 128, and bounds FAT root traversal.
It stores findings only in a new ignored `.local/` directory. It does not recurse
through the vendor tree or issue filesystem writes.

Read success does not grant write ownership. Empty sectors or gaps would not prove
that stock ignores them. Settings and rides remain in their existing explicit
`ota_1` reservations. A future bulk backend must establish stock's resource/update use beyond the bounded root listing
and obtain an
explicitly owned namespace or region with bounded and recoverable writes. Until
then, no cycling code writes this MMC device.

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
