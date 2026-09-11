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

The firmware remains pinned to `esp-hal` 1.1.2. Its small ESP32-S3-only probe is
adapted from the upstream 1.2.1 driver and uses the matching PAC already required
by the GNSS DMA path. It is compiled in only with `CYCLING_SDMMC_PROBE=1`; ordinary
firmware reserves the recovered peripheral and pins but does not initialize the
medium.

The probe uses slot 1, one-bit width, 400 kHz and 3.3 V signaling. It issues only
the audited identification, selection and read commands. On this unit SD CMD8 and
CMD55 did not respond; after a separate CMD0, MMC CMD1 did. The probe read EXT_CSD,
sector zero twice and the final reported sector. Both sector-zero reads matched,
and all three bounded reads completed on two boots with the same private hashes.
Every transfer stops or resets IDMAC before its aligned internal buffer leaves
scope. It does not mount, format, repair, erase or write the medium.

After probing, `mise run stock` verified and selected the preserved stock slot,
then `mise run flash` restored the harness-enabled cycling application. The stock
application does not expose a USB startup log through this harness, and no camera
was available, so this proves the safe selector/verification path rather than a
visual stock-screen or stock-filesystem test. The restored cycling application
reported five rides at slot 30, unchanged preferences, working display/touch,
verified Wi-Fi and time, advancing GNSS/companion data and the selected HRS link.

Read success does not grant write ownership. Empty sectors or gaps would not prove
that stock ignores them. Settings and rides remain in their existing explicit
`ota_1` reservations. A future bulk backend must first map the partition and
filesystem read-only, establish stock's resource/update use, and obtain an
explicitly owned namespace or region with bounded and recoverable writes. Until
then, no cycling code writes this MMC device.

## Settings journal in owned slot B

Cycling firmware owns stock partition `ota_1`, from `0x760000` through `0xe9a000`.
The application is now limited to `0x738000` bytes, ending at `0xe98000`. Its final
two 4 KiB erase sectors, `0xe98000..0xe9a000`, hold a small settings journal. The
device flasher rejects an application image that reaches this reserved area.

Each sector contains a format version, sequence, bounded payload length, CRC-32,
and a commit word. A write erases the inactive sector, writes its record while the
commit word remains erased, then writes the commit word last. Reads validate every
field and select the newer valid sequence. A torn erase, record write, commit, or
checksum failure leaves the other committed sector available. The usable payload
capacity is 4,072 bytes. Errors from flash reads, erases, writes, output bounds,
and readback validation are reported explicitly.

This area is intended for small settings records. It is not bulk ride storage.
Installing stock firmware into slot B or another tool writing the complete
`ota_1` partition can erase it. The project flasher writes only the bounded cycling
application extent, so routine cycling reflashes preserve the journal.

## Hardware validation (2026-09-11)

The harness-enabled 557,456-byte application was flashed twice with `mise run
flash`. The safe workflow verified the bootloader, partition table and stock slot
before each write and verified both application slots afterward. On the first boot,
firmware reported that it created and read back sequence 1 with an eight-byte
payload. On the second boot after rewriting the application, it loaded the same
sequence and length without initializing the journal. Both boots continued through
touch, display and Wi-Fi startup, and no storage error was reported.

Host tests cover the full 4,072-byte payload, overflow and short-output errors,
checksum fallback, and partial erase, record-write and commit-word interruptions.
The interrupted cases replace a previously committed sector and recover either the
previous complete payload or the new complete payload, never partial bytes.
