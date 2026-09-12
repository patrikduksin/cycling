# Hardware

**Verified** means observed on the device. **Recovered** means traced in stock code.
**Candidate** means a supported driver or diagnostic name, not a fitted-part ID.

| Function | Evidence | Status |
|---|---|---|
| Main MCU | ESP32-S3 QFN56 rev 0.2; two LX7 cores; 512 KiB internal SRAM | Verified chip, published SRAM size |
| PSRAM | 2 MiB in-package Quad SPI RAM at 40 MHz | Rust initialization and full-range startup test verified |
| Boot flash | 16 MiB; JEDEC manufacturer `c8`, device `4018` | Verified full readback |
| LCD | ST7789-compatible, 240×320, 16-bit I80 | Independent C display confirmed |
| Backlight | GPIO45, LEDC 20 kHz, 10-bit PWM | Independent C test confirmed |
| Wi-Fi | ESP32-S3 integrated 2.4 GHz radio | Rust WPA2 association, DHCP, DNS, public HTTP and reconnect verified |
| BLE | ESP32-S3 integrated radio; stock ESP-IDF driver paths | Echo/reconnect verified, including forced MTU 23; see PR #71 |
| Companion | Official N22 update identifies `NRF52810_APP`; Nordic/ANT implementation | Update identified; chip readback pending |
| Touch | I2C0, SDA21, SCL12; `0x5a`, packed coordinates at `0xd000` | Rust touch and visual alignment confirmed; exact part ID pending |
| Buttons / power | UART2 RX41 at 115200; all three short-click IDs mapped | Three buttons and brightness shortcuts physically confirmed; power control pending |
| Battery / charge | Companion streams voltage, percentage and power status | Percentage and USB power transition physically confirmed; calibration pending |
| Sound | Companion buzzer control; main audio-resource management | Buzzer lead; speaker/codec unconfirmed |
| GNSS | UART0 RX0 at 921600; live GN NMEA | Verified receive path; exact receiver and control effect pending |
| Motion | Companion `icm42607` and `qma6100` ID checks | Variant candidates |
| Pressure | Companion `spl0601`, `spl06001`, `spa06003` diagnostic names | Variant candidates; spelling preserved |
| Resource storage | SD/MMC, FAT, `/sdcard` mount | MMC/eMMC identification and one-bit reads verified; see storage.md |

## Display wiring

| Signal | GPIO |
|---|---|
| CS / DC / WR / RD | 2 / 40 / 3 / 39 |
| D0–D7 | 4, 38, 5, 37, 6, 36, 7, 35 |
| D8–D15 | 8, 34, 9, 33, 10, 47, 11, 48 |
| Backlight | 45 |

RD stays high. Display initialization comes from the main routine at `0x4202aa2c`
and ST7789 configuration at `0x4202aec4`. The tested unit's board-version predicate
does not enable optional GPIO43/44 handling. Do not copy pin assignments to another
board revision without checking.

Touch initialization at `0x4202ad98` tries address `0x38` and registers `a8`, `00`,
`a6`, `af`; its fallback at `0x5a` uses register `d045`. Address alone does not identify
a controller. Main I2C configuration begins at `0x4202b438`.

Stock firmware uses UART0 TX1/RX0 for GNSS and UART2 TX42/RX41 for the companion.
The custom firmware has verified both receive paths. It completed a UART2 write
on TX42, but external reception was not verified. The GNSS TX1 electrical path
has not been exercised by the custom firmware.

Sources: [ESP32-S3 datasheet](https://www.espressif.com/sites/default/files/documentation/esp32-s3_datasheet_en.pdf),
N21/N22 update analysis, USB ROM inspection and independent display tests.

## GNSS evidence and limits

The connected unit produces checksum-valid GN talker NMEA on UART0 RX0 at the
stock startup rate of 921600 baud. A passive capture received GGA, GLL and GSA
sentences before the custom firmware sent any control command. The indoor unit
reported quality 0, zero satellites and no position. The exact receiver model,
antenna state and any adaptive baud behavior remain unknown.

Stock analysis recovered UART2 TX42/RX41 at 115200 and one complete 16-byte
companion command used by its GPS open path. The custom firmware sends that
candidate once at startup. The live NMEA stream existed before the command, and
no acknowledgment or change in acquisition state was observed after it. This
confirms only that the candidate frame was sent. It does not verify power or
enable semantics. The custom firmware does not send the recovered close or
reset-like commands and does not change unverified GPIOs.

Current acquisition uses UART0/UHCI DMA and an independent Embassy task, with
resynchronization after reported transport loss. The parser distinguishes no data,
no fix, fresh and stale fixes and matches satellite data to the coordinate epoch.
The owning code is [gps_uart.rs](../../os/src/device/gps_uart.rs),
[positioning.rs](../../os/src/services/positioning.rs) and [gps.rs](../../os/src/gps.rs).

Early display-coupled reception suffered UART losses during network and UI work.
That implementation is retired. [#34](https://github.com/patrikduksin/cycling/issues/34)
contains the subsequent investigation and user-confirmed outdoor functionality.
[PR #71 evidence](https://github.com/patrikduksin/cycling/pull/71#issuecomment-5647388096)
records indoor parser progress and recovery after deliberate executor starvation.
Neither establishes receiver identity, electrical enable semantics or measured
outdoor accuracy. Raw NMEA and coordinates remain private.

## PSRAM

USB ROM inspection identifies the connected QFN56 chip as an ESP32-S3 with
2 MiB of embedded 3.3 V PSRAM. [Espressif documents the S3R2 package as Quad
SPI](https://docs.espressif.com/projects/esp-faq/en/latest/software-framework/peripherals/spi.html);
the 8 MiB S3R8 package uses Octal SPI. The Rust firmware selects Quad SPI
explicitly at 40 MHz and asks `esp-hal` to detect the capacity from the PSRAM
chip ID. The connected unit reported 2,097,152 bytes.

Startup detected and tested the full 2,097,152-byte range on two reset boots.
The integrity test operates through the cache, not directly on package data pins.
Its implementation and failure policy live in [psram.rs](../../os/src/device/psram.rs).

PSRAM uses a separate external-only allocator. A startup probe allocated 64 KiB
with 64-byte alignment, checked both ends and returned it successfully. Ordinary
global allocations, task stacks, atomics, the radio and the current LCD DMA
buffers remain in internal RAM. PSRAM is cache-backed and cannot hold data that
must remain accessible while the external-memory cache is disabled. `esp-alloc`
also warns that ESP32-S3 atomic operations do not work correctly in PSRAM.

## Touch and backlight

The connected unit acknowledges `0x5a` on I2C0 at 100 kHz. Register `0xd045`
returns four bytes `04 00 00 01`. The recovered `0x38` probes receive address
NACKs. These observations establish the working address, not an exact part ID.

Stock's routine at `0x4202b850` reads seven bytes from `0xd000` and acknowledges
with `d0 00 ab`. Coordinates use byte 1 and the high nibble of byte 3 for X,
byte 2 and the low nibble of byte 3 for Y. Low nibble 6 of byte 0 means pressed.
The format agrees with the [Hynitron CST3240 application manual](https://www.buydisplay.com/download/ic/CST3240_Application_Manual.pdf),
but CST3240 remains a controller candidate, not a verified fitted part.

The current [input service](../../os/src/services/io.rs) polls independently of
display submissions. It validates one-finger coordinates and leaves controller
firmware, calibration and reset alone. Brightness preferences are now persisted;
the former slider and per-frame polling are retired.

On 2026-09-11 the user confirmed accurate finger tracking and visible brightness
changes in the retired touch UI. This was a physical observation, not a full-panel
calibration or luminance measurement. The earlier Rust coin demo had physically
confirmed colors and smooth rotation on 2026-09-08. Current terminal display
completion alone does not repeat those physical observations.

Button and battery packet details are in [the companion notes](companion.md).
