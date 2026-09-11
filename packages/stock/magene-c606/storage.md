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

Card-detect and write-protect are disabled in that configuration. These values are
recovered driver candidates. Cycling firmware has not initialized the bus or read
CID/CSD, so the wiring and whether the fitted device is SD or MMC/eMMC remain
unverified. The first hardware investigation must stay at the read-only block
layer and must not format, repair, or mount the vendor filesystem read-write. That
work is tracked in [issue #21](https://github.com/patrikduksin/cycling/issues/21).

The current firmware pins `esp-hal` 1.1.2, which does not include its later SDMMC
driver. Updating the HAL also changes the radio dependency set. Issue #2 therefore
does not add a private register-level driver merely to probe this medium.

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
