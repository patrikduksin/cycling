# Device workflow

These tasks support the recovered C606 layout, with secure boot and flash encryption
disabled. Connect USB and start on stock firmware. The default port is `/dev/ttyACM0`;
set `CYCLING_PORT` to use another port.

```sh
mise run backup   # one-time, verified full 16 MiB backup; reboots stock
mise run flash    # builds, verifies stock, writes slot B, selects it, reboots
mise run monitor  # USB output, without resetting
mise run stock    # verifies and selects stock slot A, reboots
```

Backups, device identity and logs stay in ignored `.local/device/`. Keep a separate
copy of that directory. The helper refuses to overwrite an existing baseline backup.
It checks the connected device, partition layout, bootloader and stock image before
changing firmware. A candidate must pass ESP image checksum/hash and size checks.
Flash writes are limited to slot B and one OTA selection sector; readback validates
the change. No bootloader, partition-table, settings or eFuse writes are requested.

On Linux, the serial device needs read/write permission. A temporary ACL is sufficient:

```sh
sudo setfacl -m "u:$(id -un):rw" /dev/ttyACM0
```

Reconnection can remove that ACL. Never run generic `espflash flash` or `idf.py flash`
against this device: their defaults can replace the bootloader or stock slot.

Device tasks hold an exclusive local lock so monitoring cannot race a flash command.

After reset, the helper opens USB with DTR/RTS released and captures three seconds
of boot output in `.local/device/`. On the tested C606, stock stayed dark until this
port-open step. A completed reset command alone does not confirm startup; check
the screen. The underlying reset-line behavior is not yet established.

The helper is specific to this board layout. It is not a universal recovery tool.
An interrupted operation may leave the ESP in download mode; reconnect and inspect
its private logs, then use the stock task with the verified backup.

## What has been demonstrated

On one C606 with stock release 1.956:

- USB ROM download mode, full-flash readback and device digest verification.
- Booting a modified stock application from slot B, then returning to stock.
- Booting an independent C application and displaying text and animated graphics.
- Rust `no_std` firmware rendering a smoothly spinning coin, physically confirmed.
- Rust touch test UI with finger tracking and a 5–100% brightness slider,
  physically confirmed on 2026-09-11. Brightness starts at 50% after reboot.
- Three physical button counters and brightness shortcuts, plus battery status
  changing correctly through USB unplug/replug, physically confirmed on 2026-09-11.
- Original stock slot A preserved throughout.

The Rust demo's hardware result is recorded in [the C606 notes](../packages/stock/magene-c606/README.md).
Other board revisions may have different components or pin assignments.
