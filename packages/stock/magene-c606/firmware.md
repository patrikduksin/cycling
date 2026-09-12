# Stock firmware and boot

## Update container

Validated against N21 releases 1.410/1.956 and N22 releases 1.628/1.902.

1. The final two bytes are a little-endian CRC16/XMODEM over preceding bytes.
2. XOR those preceding bytes with repeating `9a 92 45 42`.
3. The first 128 decoded bytes form a wrapper, beginning `a5 5a 55 aa`.
4. Little-endian u32 at wrapper offset `0x14` is payload length. File length is
   payload length + 130 for the four examined files.
5. XOR the remaining payload with repeating wrapper bytes `6..8`.

No compression is involved. The wrapper's other fields are only partly understood.
The unpacker's strict length check is an analysis check; the stock updater instead
uses file length minus 130 in the path examined.

N21 yields an ESP32-S3 application with valid ESP segment checksum and appended
SHA-256. N22 yields ARM Thumb firmware whose header identifies NRF52810_APP; its
flash base is `0x12000`. Neither an XOR wrapper nor an unkeyed checksum authenticates
a firmware publisher.

Stock uses ESP-IDF, FreeRTOS and LVGL. Unpacked applications do not contain the
original debug symbols, partition table or full recovery environment.

## Boot and recovery

The tested ESP32-S3 reports secure boot and flash encryption disabled. Its original
bootloader is at `0x0`, partition table at `0x8000`, OTA selection at `0xd000` and
`0xe000`. The two application slots are `0x20000` and `0x760000`, each `0x73a000` bytes.
See [partitions.csv](partitions.csv) for the complete layout.

OTA records store sequence at offset 0, state at 24 and CRC32 at 28. CRC is calculated
with initial value `0xffffffff` over the four sequence bytes. For two slots, a valid
sequence selects `(sequence - 1) % 2`; the highest valid sequence takes priority.
Invalid/aborted records are excluded. Existing stock records use state `0xffffffff`.

The device has demonstrated USB ROM entry, full 16 MiB backup/verification, a modified
stock image, independent C firmware, and return to stock through OTA selection.
That is evidence for this physical unit, not a statement about every C606's security
configuration. The full backup does not include the Nordic flash or SD/MMC storage.

On 2026-09-12, `mise run stock` selected ota_0 and verified the preserved image,
but its RTS hard-reset left the ROM reporting `DOWNLOAD(USB/UART0)` and
`waiting for download`. Changing only the reset to esptool's `watchdog-reset`
booted stock into its normal state, confirmed by the user. USB disappeared after
that reset, so serial capture alone could not confirm application startup.
The stock task, also named `boot-stock`, now uses watchdog reset. See
[esptool reset modes](https://docs.espressif.com/projects/esptool/en/latest/esp32s3/esptool/advanced-options.html#reset-after-operation-after).

The main stock updater's OTA routine was located at `0x421b3488`; it performs begin,
write, end and boot-selection operations. Stock download handling also checks MD5.
Our USB flashing workflow does not use that vendor download path.

References: [ESP image format](https://docs.espressif.com/projects/esptool/en/latest/esp32s3/advanced-topics/firmware-image-format.html),
[ESP-IDF startup](https://docs.espressif.com/projects/esp-idf/en/stable/esp32s3/api-guides/startup.html).
