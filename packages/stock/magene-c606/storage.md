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
mode. The CID product field is `004GA1`; EXT_CSD reports
3,959,422,976 bytes in 512-byte sectors. The raw CID, CSD, sector hashes and read
traces remain private.

The optional read-only probe is enabled with `CYCLING_SDMMC_PROBE=1`; ordinary
firmware reserves the peripheral and pins without initializing the medium.
Its implementation is in [sdmmc.rs](../../os/src/device/sdmmc.rs).

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

Read success does not grant write ownership. Empty sectors or gaps would not prove
that stock ignores them. Settings and rides remain in their existing explicit
`ota_1` reservations. A future bulk backend must first map the partition and
filesystem read-only, establish stock's resource/update use, and obtain an
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
