---
name: new-device
description: Check whether a visitor's bike computer can run cycling firmware, given a cycling.computer connect code. Use when asked to connect to a code, inspect or survey a device, or judge whether we can flash it.
---

# New device

A visitor plugs their device into Chrome at https://cycling.computer and reads
you a code. You reach it through the bridge and find out what it is, whether
its ROM bootloader is reachable, and whether it accepts our unsigned firmware.

The device-safety rules in [AGENTS.md](../../../AGENTS.md) apply. In addition:

- The visitor owns the device and watches the page. Never write to it in this
  skill. A write needs Patrik's go-ahead in the task and runs through the
  [C606 skill](../c606/SKILL.md) flows.
- Never print the MAC or a serial number. `cycling flasher info` prints
  `device_id` (a keyed hash) in its place; use that as the device's name.
- A flash dump holds the owner's data (settings, Wi-Fi credentials). Keep it
  under `.local/`, never commit, upload or quote it.
- Leave the device as you found it: running its own app.

## Steps

1. **Join.** Run `cycling remote <CODE>` in the background. It prints
   `CYCLING_PORT` (the device's serial console) and `CYCLING_FLASHER`, then a
   status line with the USB VID:PID. Export both variables for the commands
   below. Done when the status line says `port open`.
2. **Listen.** `cycling monitor --seconds 10` captures what the stock firmware
   prints. Send nothing to firmware you don't know. Done when you have the
   capture, even if it is empty.
3. **Enter the ROM.** `cycling flasher info` puts the chip in download mode,
   prints its description as JSON and reboots it into its app. Success means
   the ROM bootloader is reachable from the browser. On failure, keep the
   error: it names the reason (`unsupportedChip`, `entryFailed`, `portLost`).
   Every `cycling flasher` verb enters and reboots the same way.
4. **Dump.** `cycling flasher read 0 <flash.size> .local/survey/<device_id>/flash.bin`.
   The chip checks every piece against its own digest. Expect about 2 minutes
   per 16 MiB. Done when the command exits 0 and the file has the full size.
5. **Boot log.** `cycling flasher reboot --seconds 5` saves the boot output
   under `.local/device/flasher/`. The ROM and bootloader lines name the chip
   revision, flash mode and the partition the bootloader picks.
6. **Read the dump offline.** Every question below has an answer or an
   explicit "unknown":
   - Partition table at `0x8000`: 32-byte entries starting `AA 50`, ending at
     `EB EB` or `FF FF`. List label, type, subtype, offset, size.
   - Each app partition: extract it and run `python -m esptool image-info`
     on it for chip, entry point and segments. The ESP-IDF app description
     (magic `0xABCD5432` in the first segment) gives project name, version,
     IDF version and build date.
   - Hardware hints: `strings -n 6` over the apps for display, touch, GNSS,
     ANT+/BLE, sensor and PMIC part numbers and board names.
   - OTA layout: whether two app slots exist and how large they are, against
     our image size in `.local/cycling.bin`.
7. **Report.** Write `.local/survey/<device_id>/report.md` and reply to
   the user with the same content:
   - Chip family, model, revision, features, crystal, flash size and JEDEC id,
     USB VID:PID.
   - Bootloader reachable from the browser: yes or no, with the step that
     proved it.
   - Unsigned firmware accepted: `accepts_unsigned` from `info`. `null` means
     the ROM cannot say (classic ESP32); write it as unknown.
   - Partition layout and the stock app's identity.
   - Hardware hints, each marked as inferred from strings.
   - Verdict: can cycling firmware run here, and what a port needs (board
     support, drivers, display, inputs).

## What this cannot tell

Pin assignments and which peripheral sits on which bus are not in the
flash in plain form. Say so, and list them as what a port must find.
