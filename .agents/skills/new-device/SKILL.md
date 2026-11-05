---
name: new-device
description: Check whether a visitor's bike computer can run cycling firmware, given a cycling.computer connect code. Use when asked to connect to a code, inspect or survey a device, or judge whether we can flash it.
---

# New device

A visitor plugs their device into Chrome at https://cycling.computer and reads
you a code. You reach it through the bridge and find out what it is, whether
its ROM bootloader is reachable, and whether it accepts our unsigned firmware.

The device-safety rules in [AGENTS.md](../../../AGENTS.md) apply. In addition:

- The visitor owns the device and watches the page. Never write to it.
  Writing to a visitor's device is not supported yet (#348); the `cycling
  flash` flows refuse any device but the backed-up C606.
- Every device command goes through `cycling remote <CODE> -- <command>`.
  Run bare, `cycling flasher` and `cycling monitor` fall back to a locally
  attached device, which may be Patrik's C606.
- Never print the MAC or a serial number. `cycling flasher info` prints
  `device_id` (a keyed hash) in its place; use that as the device's name.
  Console and boot logs can hold both: keep them in files, describe them,
  and never quote MAC- or serial-shaped strings from them.
- A flash dump holds the owner's data (settings, Wi-Fi credentials). Keep it
  under `.local/`, never commit, upload or quote it.
- Leave the device as you found it: running its own app.

## Steps

Run every command from the repository root; `.local/` below is that
checkout's.

1. **Join and listen.** `cycling remote <CODE> -- cycling monitor --seconds 10 > .local/survey/<CODE>/console.log`.
   The bridge prints the device's USB VID:PID on stderr, waits for the
   visitor's port, then the monitor captures what the stock firmware prints
   into the file (create `.local/survey/<CODE>/` first). Send nothing to
   firmware you don't know. Done when it exits
   0, even with an empty capture.
2. **Enter the ROM.** `cycling remote <CODE> -- cycling flasher info` puts the
   chip in download mode, prints its description as JSON and reboots it into
   its app. Check that its `usb` ids match the VID:PID from step 1. Output
   without a `device_id` means the visitor shared the device only as a USB
   descriptor: the bootloader is not reachable from the browser. Success
   means the ROM bootloader is reachable from the browser. On failure, keep
   the error: it names the reason (`unsupportedChip`, `entryFailed`,
   `portLost`). Every `cycling flasher` verb enters and reboots the same way.
3. **Dump.** `cycling remote <CODE> -- cycling flasher read 0 <size> .local/survey/<device_id>/flash.bin`,
   with `<size>` the smaller of `flash.size` and `0x1000000`: the page reads
   at most 16 MiB, so mark anything above as unread. The chip checks every
   piece against its own digest. Expect about 2 minutes per 16 MiB. Done when
   the command exits 0 and the file has the full size.
4. **Boot log.** `cycling remote <CODE> -- cycling flasher reboot --seconds 5`
   saves the boot output and prints its path. Chrome reopens the port only
   after the chip re-enumerates, so the first ROM and bootloader lines are
   usually missing; expect the app's own start-up output.
5. **Read the dump offline.** Every question below has an answer or an
   explicit "unknown":
   - Partition table at `0x8000`: 32-byte entries starting `AA 50`, ending at
     `EB EB` or `FF FF`. List label, type, subtype, offset, size. If `0x8000`
     holds no entry, search up to the first app image (`E9` at a 64 KiB
     boundary) for an `AA 50` record before calling the table missing.
   - Each app partition: an image starts with `E9`. Run `python -m esptool
     image-info` on the extracted image for chip, entry point and segments.
     The ESP-IDF app description sits at image offset 32 (magic
     `0xABCD5432`): version at +16, project at +48, time at +80, date at +96,
     IDF version at +112, each a NUL-padded string.
   - Hardware hints: `strings -n 6` over the apps for display, touch, GNSS,
     ANT+/BLE, sensor and PMIC part numbers and board names.
   - OTA layout: whether two app slots exist and how large they are, against
     our image size in `.local/cycling.bin`.
6. **Report.** Write `.local/survey/<device_id>/report.md` and reply to
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
