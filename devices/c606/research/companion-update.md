# Companion update experiment

The N22 application update sender lives in
[companion_update.rs](../src/drivers/companion_update.rs). Its UART interpreter
is in [companion_uart.rs](../src/drivers/companion_uart.rs); the host observes
it through `cycling flash --companion-update` (or `cycling device flash` on a
remote device). Vendor packages, decoded images, disassembly and captures stay
in ignored `.local/`.

## Local verification, 2026-11-06

The recovered N21 1.956 updater sends a group-one entry request, file information,
128-byte decoded image packets, then app-mode queries. There is no final CRC or
activation message. Each request waits for its matching acknowledgment. The
sender gives every retry a full timeout; stock cuts its final retry short.
The final stock whole-device power-cut command is deliberately omitted. The
stock companion's own application startup can nevertheless restart the ESP32,
as observed below.

All 617 generated update frames matched the independently recovered reference
sequence byte for byte: entry, file information, all 614 data packets, and the
final query. The decoded image is 78,504 bytes, with an unpadded 40-byte last
packet. Host tests exercise short, negative, duplicate and stale acknowledgments,
bounded retries, checksum rejection, and the update-only reply-direction
compatibility. Observer tests reject stale app-mode evidence and never replay
an uncertain start.

The stock-only experiment embeds the verified stock N22 1.902 image. A separate
[guarded marker trial](companion-custom-trial.md) derives one fixed patch from it. The build script
checks both the original package and decoded image SHA-256 before enabling the
update commands. Ordinary builds have no embedded image and cannot start an
update. Set `CYCLING_COMPANION_STOCK` to the private original container when
building an experiment image; this requires `CYCLING_HARNESS=1`.

To repeat the private reference check, set `CYCLING_COMPANION_DECODED` to the
independently unpacked image and `CYCLING_COMPANION_VECTORS` to the recovered
frame-list file, then run `cycling companion-update-check`.

Once started, the ESP32 retains the full image and runs the transfer independently
of the USB observer. All other companion writes and new power requests are
blocked until the main CPU is deliberately restored or reset. UART FIFO writes
are partial and bounded; a transmit stall fails the session. A timeout or
disconnect is an uncertain result: inspect `TEST 43` and private evidence before
resetting or attempting another write. `TEST 41` retains recent non-report replies.

## Hardware observation, 2026-11-06

The experiment ran on the connected C606, base firmware at `4ff4bb1de217` with
uncommitted uploader/probe changes, `CYCLING_SDK=0`, `CYCLING_HARNESS=1`, slot B.
The main application image was SHA-256 checked and flashed through the ordinary
A/B workflow, with its ride/settings reservations verified unchanged.

A fresh app-mode reply preceded the single update-start submission. The
companion acknowledged entry, file information and image packets. The last
USB observation was packet index 602 of 614, with no retries for that packet,
at main uptime 21.270 s. USB then disconnected near the end of the transfer.
The observer stopped and did not replay the start.

USB returned without physical intervention. Main uptime restarted and STATUS
reported `reset=power`, `crash=none`. A newly sent mode query received a fresh
app-mode answer. Read-only checks then observed fresh motion and battery
reports, zero companion CRC/UART errors, and idle/ready power control. The
uploader's RAM state had reset to Ready. This supports a completed stock update
followed by a companion-driven main power cycle. It does **not** capture the
last packet acknowledgment or the updater's Complete state; those were lost
with the main power cycle. No byte-for-byte companion flash readback was done.

The stock main application and vendor filesystem were never booted or written
for this experiment. No forced interrupted-update or broken-image recovery
test was attempted. Treat a future disconnect as uncertain until similarly
checked with read-only commands; a new DFU submission is not a reconnect action.

After these checks, `cycling device rollback` returned to `dd35129ddf28` in
slot A. That image predates the base/cycling split, so a final ordinary
`cycling device flash`, with `CYCLING_COMPANION_STOCK` unset, installed the
current explicit SDK0/harness1 base at `4ff4bb1de217` with the working-tree
changes in slot B. INFO confirmed `cycling=false harness=true`. This final
image has no embedded companion firmware and its update trigger is disabled.
All readers were closed. `cycling test`, `cycling check`, the private frame
comparison and observer tests, and all four SDK/harness firmware combinations
passed locally.

## Power-control correction from the stock desk study

Earlier hypotheses assigning Nordic GPIO23 and GPIO13 to ESP32 power were
incorrect. Stock uses those lines, together with GPIO22 and the board-dependent
GPIO15 sequence, in its GNSS open/close/reset handler. ESP32 power/enable is a
separate indexed control (control 5); the DFU handoff does not deliberately
toggle it. These are code-derived assignments, not electrical measurements.
Subsequent decoding of the complete application resolved control 5 to P0.7
and the ESP32 boot strap to P0.24; see the
[closed-case recovery investigation](companion-recovery-sources.md).

## Limits

A same-version stock update does not establish that an unsigned custom image
will be accepted, that a crashing custom application can be recovered over USB,
or that the ESP32 can power up independently of the companion. The bootloader
itself has not been read back. SWD access and forced recovery from an application
that disables ESP32 power or UART remain untested. Low-power motion wake and
standby current have not been implemented or measured by this experiment.
