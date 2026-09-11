# Hardware

**Verified** means observed on the device. **Recovered** means traced in stock code.
**Candidate** means a supported driver or diagnostic name, not a fitted-part ID.

| Function | Evidence | Status |
|---|---|---|
| Main MCU | ESP32-S3 QFN56 rev 0.2; two LX7 cores; 512 KiB internal SRAM | Verified chip, published SRAM size |
| PSRAM | Chip identifies 2 MiB in-package PSRAM | Identified; custom bring-up pending |
| Boot flash | 16 MiB; JEDEC manufacturer `c8`, device `4018` | Verified full readback |
| LCD | ST7789-compatible, 240×320, 16-bit I80 | Independent C display confirmed |
| Backlight | GPIO45, LEDC 20 kHz, 10-bit PWM | Independent C test confirmed |
| Wi-Fi / BLE | ESP32-S3 integrated radios; stock ESP-IDF driver paths | Custom radio tests pending |
| Companion | Official N22 update identifies `NRF52810_APP`; Nordic/ANT implementation | Update identified; chip readback pending |
| Touch | I2C0, SDA21, SCL12; `0x5a`, packed coordinates at `0xd000` | Rust touch and visual alignment confirmed; exact part ID pending |
| Buttons / power | Companion button events, combinations and power control | Protocol pending |
| Battery / charge | Companion voltage, capacity, charging and temperature handling | Circuit and calibration pending |
| Sound | Companion buzzer control; main audio-resource management | Buzzer lead; speaker/codec unconfirmed |
| GNSS | NMEA and adaptive receiver logic; PAIR/PDTINFO/CCMSG/CFGMSG command families | Exact receiver and UART path pending |
| Motion | Companion `icm42607` and `qma6100` ID checks | Variant candidates |
| Pressure | Companion `spl0601`, `spl06001`, `spa06003` diagnostic names | Variant candidates; spelling preserved |
| Resource storage | SD/MMC, FAT, `/sdcard` mount | Medium, capacity and pins pending |

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

Generic UART setup has branches UART0 TX1/RX0 and UART2 TX42/RX41. Their assignment
to each external component remains unresolved. These are not identified debug pads.

## Next measurements

Identify the exact touch controller, decode companion status/button packets, capture GNSS
identity and position data, read storage CID/CSD, then exercise wireless. Keep the
companion firmware initially; accessing its host protocol may expose several
functions without reimplementing its sensor and power-management drivers.

Sources: [ESP32-S3 datasheet](https://www.espressif.com/sites/default/files/documentation/esp32-s3_datasheet_en.pdf),
local N21/N22 update analysis, USB ROM inspection and the independent C display test.

## Touch and brightness bring-up, 2026-09-11

The connected unit acknowledges `0x5a` on I2C0 at 100 kHz. Register `0xd045`
returns four bytes `04 00 00 01`. The recovered `0x38` probes receive address
NACKs. These observations establish the working address, not an exact part ID.

Stock's routine at `0x4202b850` reads seven bytes from `0xd000` and acknowledges
with `d0 00 ab`. Coordinates use byte 1 and the high nibble of byte 3 for X,
byte 2 and the low nibble of byte 3 for Y. Low nibble 6 of byte 0 means pressed.
The format agrees with the [Hynitron CST3240 application manual](https://www.buydisplay.com/download/ic/CST3240_Application_Manual.pdf),
but CST3240 remains a controller candidate, not a verified fitted part.

Our Rust implementation polls once per 42 ms frame, accepts one finger, and
rejects malformed or out-of-panel coordinates. It leaves controller firmware,
calibration and reset alone. The UI shows touch position and a slider which
sets GPIO45 PWM duty from 5 to 100 percent. It starts at 50 percent after each
boot; settings are not persisted. A drag must start in the slider area to change
brightness. Lost reports cancel an active touch after 250 ms.

USB captured live drag coordinates and corresponding successful PWM duty updates,
including 11 through 71 percent during the initial boot capture. Frame work was
19 ms. The user confirmed that the marker follows their finger accurately and
that the slider visibly changes brightness, describing both as perfect. This is
a user-observed hardware result, not a measured full-panel calibration or
backlight luminance measurement. Private captures remain in `.local/device/`.
