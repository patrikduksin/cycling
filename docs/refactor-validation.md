# Refactor validation, September 12, 2026

The refactored base and optional SDK passed the supported build matrix and
observed device checks. The C606 was left running base firmware
`660affdce26e`, harness enabled, SDK disabled, INFO logging and ordinary terminal
available. [Architecture](architecture.md) describes ownership and future-port
obligations; [the durable handoff](overnight-plan.md) records publication state.

## Builds and verification

The last firmware-logic change in this checkpoint is `8bc6482`. Later changes
through `9817bf1` and the base checkpoint `660affd` concern documentation and
host tooling. Hardware INFO records identified base-disabled firmware as
`8bc648235918` and both SDK modes as `9817bf18b2e5`. Keep those actual reported
revisions distinct from the latest Git checkout. The final base-enabled device reports `660affdce26e` with a clean build.
Subsequent validation changes affect host tools, CI and documentation only.

Host checks passed for 54 base tests, 87 cycling-SDK tests, 15 pinned Trouble Host
tests and 55 Python tests. Formatting, Clippy, boundary checks and all four
firmware build combinations passed; GitHub's host and four firmware jobs also
passed for the merged service work. Base compilation excludes SDK/UI imports;
feature-enabled tests retain sensor, recording, recovery and export coverage.

| Application image | SDK | Harness | Bytes | Reduction from old 856,224-byte image |
| --- | --- | --- | ---: | ---: |
| Base enabled | off | on | 803,552 | 52,672 |
| Base disabled | off | off | 802,688 | 53,536 |
| SDK enabled | on | on | 835,792 | 20,432 |
| SDK disabled | on | off | 834,960 | 21,264 |

These are application-image sizes from the final four build logs. They are not
heap, stack-use or power measurements. Separate ELF section totals were retained
privately; linker reservations must not be presented as measured peak use.

## Observed device behavior

All tests used the authorized C606 and safe repository flashing, with stock
slot A, bootloader, partition table, eFuses and existing persisted data protected.
The device was indoors. The user confirmed the existing GPS implementation works
well. The new evidence below establishes
parsing/progression and indoor no-fix behavior, not new outdoor accuracy.

### Base, harness disabled

A 20-second no-render window advanced GPS by 36,864 bytes and 423 valid sentences,
and companion decoding by 672 reports. All tracked GPS checksum/parse/DMA/line/UART
and companion CRC/UART/touch/input-loss deltas were zero. Ordinary INFO, STATUS,
POSITION, INPUT and SETTINGS remained available. Malformed/overlong rejection,
three no-reset USB opens and display submission were exercised.

The first combined restart test failed because queued same-boot startup metadata
was misclassified as a new reboot by the host. Its report correctly remains
`ok=false`; it is not silently counted as a pass. A captured-trace regression and
independent review covered the host correction. The hardware retry passed,
advancing another 10,240 GPS bytes, 120 valid sentences and 172 companion reports
with all tracked fault deltas zero. Save/restart/readback and restoration passed;
preferences returned to brightness 100, timeout 30 seconds, dim 20 and timezone
-180 minutes. The harness-only transport test returned UNSUPPORTED in this mode.

Initial post-flash USB silence required the documented monitor-release step.
Subsequent ordinary attachments used the no-reset open path. Display submissions
reported maximum completion times of 13–14 ms in the sampled checks. This verifies
firmware/DMA completion, not physical panel output.

### SDK, harness disabled

A ten-second window advanced GPS by 18,688 bytes and 217 valid sentences, and
companion reports by 340. All tracked fault deltas were zero. Ordinary terminal
and logging worked; TEST 10 was UNSUPPORTED. An export matched the existing one
saved demo/four-slot baseline byte for byte.

### SDK, harness enabled

The owned CSC fixture supplied deterministic 60 rpm data. Fresh cadence appeared,
an explicit reconnect advanced connection count 1 to 2, and stale cadence was
omitted after the fixture stopped. These are fixture observations, not a new
physical cadence-sensor measurement. The fixture was stopped and unregistered.

Twelve samples committed while the USB connection was closed. During that
unattended interval, GPS advanced 24,576 bytes/285 valid sentences and companion
reports advanced 412, with no new tracked faults. Pause held active duration at
12,250 ms across the next roughly three seconds. Resume and finish saved 19
samples at 17,308 ms active duration. The correct final state was `saved`; one
private script assertion incorrectly expected `ready`. Continuation inspected
the state without replaying any mutation.

A second disposable live ride recovered after software restart at its last
committed 3,026 ms, retaining four samples. Final export contained three rides,
18 slots and 4,608 bytes with no invalid slots. The original 1,024-byte prefix
remained exactly intact. No ride-reservation initialization, reclaim or erase was performed.

STATUS `storage_ops`/`storage_max_ms` cover the settings-store instrumentation;
they do not count SDK ride appends. In particular, `storage_ops=0` during this
SDK test must not be described as an absence of flash writes. Committed ride
counts, slots and exported bytes are the evidence for those appends.

### BLE echo interoperability

Base echo passed two ordinary connect/discover/notify/readback sessions at the
observed MTU of 60. Two separate sessions forced MTU 23 and passed discovery,
notifications, exact readback and refusal of short writes without changing the
stored value. Independent review checked the private traces and prefix/image
evidence. The ordinary BlueZ helper reports negotiated MTU; it does not itself
force MTU 23.

## Logging, buffering and sampled overhead

All sampled modes used INFO logging and `recording=false` metadata. That field
means graphical capture is absent, not that SDK ride recording is inactive.
No screenshot or recording canvas remains. The measured target log queue occupies
3,124 bytes, including bookkeeping, for eight 384-byte slots. The terminal has
two separate 1,536-byte transmission buffers. Other formatter/state storage is
additional; these figures are not the entire RAM cost of terminal/logging.

| Collected window | Free heap | Sampled heap minimum | Max enqueue+serialization | Max active USB pump | Recorded device log loss |
| --- | ---: | ---: | ---: | ---: | ---: |
| Base disabled measurement | 80,308 B | 76,852 B | 212 µs | 277 µs | 26 |
| SDK disabled | 80,336 B | 78,584 B | 565 µs | 685 µs | 1 |
| SDK enabled collection | 80,336 B | 78,632 B | 208 µs | 175 µs | 62 |

These are individual cumulative STATUS observations, not matched workload
benchmarks. The base-disabled collector decoded 16 logs/11 replies with two
sequence gaps; SDK-disabled decoded 13/5 with no detected gap in that collection;
SDK-enabled decoded 11/6 with one gap. A device loss count can precede attachment,
so zero gaps in one host window does not prove zero device loss.

In the separate SDK recording test, STATUS sampled 80,336 bytes free and a
79,876-byte minimum, with maximum enqueue time 417 µs and active USB pump 1,036 µs.
Enqueue instrumentation excludes facade message formatting; USB pump time includes
reply traffic. Instrumentation has its own cost. Sampled minima are not allocator
high-water marks, observed maxima are not worst-case bounds, and these runs do
not isolate idle logging CPU overhead or establish power savings. See
[logging semantics](logging.md) for loss, partial records and boot boundaries.

## Reproduction entry points

Use one USB owner at a time. The public tools use `.local/usb.lock`; all output
below belongs in ignored `.local/`. USB permissions on the tested host required
`sudo -n -E`; add it only where permissions require, without changing system
security. Port selection and the monitor-release caveat are documented in
[the device workflow](device.md) and [terminal testing](device-debugging.md).

```sh
mise run test
mise run check
CYCLING_SDK=0 CYCLING_HARNESS=0 mise run build
CYCLING_SDK=0 CYCLING_HARNESS=1 mise run build
CYCLING_SDK=1 CYCLING_HARNESS=0 mise run build
CYCLING_SDK=1 CYCLING_HARNESS=1 mise run build
mise run terminal -- STATUS
mise run logs -- --seconds 10 --command INFO --command STATUS --command POSITION
python scripts/base_test.py .local/tests/base-check --seconds 20 --restart --write-settings
mise run gps-stress
mise run companion-stress
mise run bluetooth-echo
mise run ride-export
```

Choose a fresh output directory for each run. Hardware image changes must use
`mise run flash` with the desired build environment, never generic flashing.
For forced MTU 23, use `btgatt-client --mtu 23` against the privately identified
owned echo peer; do not publish its address. SDK fixture selection uses
`CYCLING_BLE_MODE=sim-csc` and `CYCLING_SIM_PROFILE=csc`.

## Final base-enabled checkpoint

All following observations use base `660affdce26e`, harness enabled, INFO logs,
no graphical capture and no ride consumer. The first 20-second no-render window
advanced GNSS by 37,632 bytes/431 valid sentences and companion by 672 reports,
with all tracked fault deltas zero.

The controlled six-second executor stall began at device 26,202 ms and ended at
32,202 ms. Position reported `failed` at 32,218 ms with one DMA loss and one UART
error; companion also reported one UART error. The position transition log
reported `receiving` at 32,958 ms. By 35,333 ms parsing had advanced another 61
valid sentences; companion had advanced 184 reports by 35,363 ms. Loss remained
visible and receiving state persisted. This validates recovery after observed
buffer starvation, not loss-free operation during an intentional stall.

The injected Wi-Fi probe failure incremented the failed-probe count from zero
to one at 37,574 ms. Clearing the fault and requesting reconnect returned to
verified state 4 at 39,699 ms, with successful probes increasing from one to two.
The raw session contains the request, failure and recovery logs alongside replies.

The first combined panic test stopped in the host after a leftover color escape
made the initial build line unstructured. The first clean boot record was then
mistaken for another reboot. The report remains failed. A captured-trace test
now establishes identity from the first valid boot token and rejects subsequent
changes; legacy reset markers remain conservative. Independent review and the
hardware panic retry passed. Controlled crash status was observed, followed by
`crash=none` after a clean restart. A continuous collector across that restart
retained 26 logs, four replies and two boot boundaries, plus raw ROM/panic text.
Host logs cannot recover records never received.

A corrected closed-host test retained the descriptor only while attached, then
closed it for 25 seconds. From 139,885 to 165,159 ms, loss increased from 95 to
164 while GNSS advanced 48,128 bytes/559 valid sentences and companion advanced
840 reports, all with zero new transport faults. Display and settings-operation
counts stayed unchanged during the closed interval. Combined session decoding
exposed sequence gaps. An earlier private script had retained a duplicate open
and used an invalid within-session gap assertion; it is not counted as a
closed-host pass. The successful run retained its own evidence separately.

For actual host-port removal, the test temporarily unbound and rebound only the
C606 tty's CDC driver interface. The collector observed port disappearance, one
disconnect, six opening retries with backoff capped at two seconds, then a second
connection and fresh INFO. Its three replies included INFO at 173,661 and
182,671 ms; ten logs retained the same boot identity. No command replay, physical
cable removal or power cycle is implied. The driver was restored.

Final ordinary collection decoded 16 logs and 12 replies. At 270,688 ms, GNSS
was receiving with no fix, 500,992 bytes/5,799 valid sentences and zero current
fault counters. Companion had decoded 8,985 reports with zero tracked faults;
Wi-Fi state was 4 with faults cleared, network time was fresh and BLE echo was
advertising. Settings remained 100/30/20/-180, with no persistence error. Final
black-fill completion brought display submissions to six, maximum 13 ms.

Cumulative final STATUS sampled 80,308 bytes free, a 79,860-byte minimum,
1,829 µs maximum enqueue/serialization, 310 µs maximum USB pump and 187 lost logs.
These mixed startup/test/idle observations are not a disabled-harness performance
baseline. Queue memory remained 3,124 bytes and PSRAM free was 2,097,152 bytes.
No terminal, collector, test process or owned BLE simulator remained at handoff.
The last SDK export verified three rides/18 slots and the original prefix before
the final protected base flash; the base has no ride writer.

## Limits

Physical panel/switch/touch accuracy, battery hardware behavior and new electrical
power measurements were not established by these terminal tests. The retained
MMC read-only result does not authorize vendor filesystem writes. #34 still owns
receiver identity, electrical control and measured outdoor reference accuracy.
BLE remains one peer; lag hidden in Trouble Host's two-entry notification queue
is not fully represented by the core drop counter. This is not a lossless sensor
stream or an unlimited-duration reliability claim.

Unknown settings journals deliberately refuse SAVE. An explicit, preserved-data
operator reset workflow is follow-up [#70](https://github.com/patrikduksin/cycling/issues/70);
no such destructive command was introduced during this refactor.
