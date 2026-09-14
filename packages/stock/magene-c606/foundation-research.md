# Companion foundation research

These findings come from static analysis of main update N21 1.956 and companion
update N22 1.902. The companion update has not been read back from the installed
unit. Addresses identify recovered entry points, not live execution evidence.
Vendor binaries, disassembly and captures remain private. The issue requirements
are [#81](https://github.com/patrikduksin/cycling/issues/81),
[#82](https://github.com/patrikduksin/cycling/issues/82),
[#83](https://github.com/patrikduksin/cycling/issues/83),
[#86](https://github.com/patrikduksin/cycling/issues/86),
[#88](https://github.com/patrikduksin/cycling/issues/88) and
[#90](https://github.com/patrikduksin/cycling/issues/90).

## Passive pressure and motion reports

Use the existing checksum-validated companion envelope. All offsets below refer
to payload bytes. Sensor reports have class 4, group `10`, and eight payload bytes.
They share page `f1` with other local reports; unknown subtypes must remain separate.

| Subtype at byte 1 | Fields | Recovered evidence |
|---|---|---|
| `01` | Three signed little-endian 16-bit values at bytes 2, 4 and 6 | N22 serializers `0x16d5c` and `0x170bc`; N21 handler `0x4220c0f0` uses signed loads |
| `02` | Three signed little-endian 16-bit values at bytes 2, 4 and 6 | N22 serializer `0x16de8`; N21 handler `0x4220c2c8` uses signed loads |
| `03` | Signed temperature in hundredths Celsius at bytes 2–3; unsigned pressure in hundredths Pa at bytes 4–7 | N22 serializer `0x171e0`; N21 handler `0x4204bd79` |

N22 pressure serialization checks the readiness accessor at `0x2244c`, then reads
compensated floating-point temperature and pressure from the structure returned by
`0x22444`. It multiplies each by 100 before integer serialization. N21 divides
each by 100. Its separate altitude calculation divides pressure by another 100
before comparing against standard reference pressure in hPa. The little-endian
32-bit reader is `0x4229e44c`. Altitude policy does not belong in the device decoder.

The pressure initialization path at N22 `0x1a058` tests register `0d` for ID `10`,
then tries an alternate path for ID `11`. It uses different configuration and
coefficient handling after identification. This establishes companion-owned
compensation and variant selection. It does not identify the installed part or
prove its factory coefficients are correct. Do not repeat those bus operations
from the main MCU or overwrite calibration.

Motion subtype 1 feeds stock's acceleration processing. Stock applies a factor
of `8 / 32767` or `32 / 32767`, selected by a resource predicate at `0x4202d440`.
That predicate reads byte 3 of `Res1Page11` and compares it to 2. Subtype 2 feeds a
separate path with factor `2000 / 32767`. Stock also applies optional calibration
and axis transforms. Those facts do not establish the fitted sensor, selected
range, physical axis orientation, or the presence of a working gyroscope on this
unit. The public diagnostic observations therefore retain signed, unscaled
wire-order triplets and distinguish the two subtypes.

The [passive decoder](../../os/src/companion_sensors.rs) rejects known reports with
incorrect lengths. Pressure plausibility limits are 10,000–130,000 Pa and
−60–100 Celsius; these are decoder guards, not fitted-part specifications or an
accuracy claim. Unknown pages cannot refresh a known sensor. UART or CRC loss
invalidates current observations; counters describe observed discontinuities and
cannot count reports lost upstream. No sensor enable or sampling-rate request was
established in this investigation. Silence means no current observation, not
unsupported hardware.

## Identity and read-only query candidates

N22 `0x16f34` constructs a ten-byte class-4/group-1/page-1 startup report. N21
`0x4204b9ac` reads bytes 7, 8 and 9, splitting byte 9 into decimal quotient and
remainder. The meaning and version notation of these fields need installed-device
corroboration. In particular, the update filename is not the installed version.
Keep the three bytes opaque when displaying the report.

N22 group-1 dispatch `0x146ae` routes page `01` to `0x15eec`, which constructs a
ten-byte reply without a peripheral or persistence operation. An eight-byte
`01 00 00 00 00 00 00 00` payload is a bounded identity-query candidate. Query
class and the reply envelope must be verified before exposing it as a completed
operation. Receiving a startup report requires no command.

Stock's `SendQuerySlaveModeCmd` at `0x420521d4` sends class 2, group 1 and payload
`24 01 ff ff 00 ff ff ff`. N22 `0x17848` has a read-only byte-1-equals-1 branch,
which returns a fixed eight-byte status response. Byte 1 equal to zero invokes
an update callback. Do not generalize this candidate into an arbitrary page-24
command API. Neither candidate establishes silicon identity.

## Buttons, battery and power

### Main startup acknowledgment and acquisition recovery

The owner-guided 2026-09-14 bench exposed a startup dependency. After a
lower-button combination and physical recovery, the main CPU, display, battery
and GNSS worked, but pressure and both motion reports stopped. Software restart
and reflashing did not restore them. The former startup code sent only GNSS-open.

N21's startup state machine sends the exact power-on acknowledgment below after
detecting companion application mode, then waits for a power-reason event. N22
consumes this acknowledgment through a one-shot RAM flag. Charging startup emits
reason 4 without initializing sensors. A subsequent normal top-left hold can
enter sensor initialization and emit reason 5; the other startup path emits
reason 6. These are state-machine observations, not electrical power measurements.
The report is class 4, group 16, with eight payload bytes
`e2 02 00 00 ff <reason> 00 ff`. Reasons and identity replies alone do not prove
acquisition readiness.

A controlled base/harness build sending that acknowledgment once received an
identity reply but still had no sensors while charging. A subsequent owner-held
top-left button with USB connected produced the stock-like startup sound and
restored fresh, independently advancing pressure and both motion streams.
Six owner-held orientations then produced opposite dominant signs for all three
components of the first motion triplet. This supports functional orientation
response; scaling, exact fitted identity and the second triplet's sensor function
remain unverified. Lifting retained fresh pressure, without an independent height
or pressure reference.

The firmware observes three seconds of companion traffic before considering this
acknowledgment. Any motion/pressure report, including malformed known pages, or
ANT traffic suppresses it. Healthy advancing sensors report `already_running`;
partial activity reports `running_degraded`. This preserves operating companion
state across main-only restarts. Absent sensor/radio traffic with intact transport permits one fixed
acknowledgment after three seconds. Battery startup does not emit periodic
battery/power reports until this acknowledgment; requiring those reports first
deadlocks startup and allows the companion acknowledgment timeout to shut down. It is never
retried after uncertain submission or timeout. `COMPANION` exposes startup state
and the raw reason. Reason 4 remains `charging` until a later operating reason;
reasons 5/6 start a bounded wait for all three sensor timestamps to advance.

This passive test is a recovery inference, not proof that every radio channel is
idle. The normal-mode acknowledgment consumer submits ANT close requests after
its reason/identity reports. Post-reason sensor progress does not prove every
asynchronous close completed, and the installed bridge retains known scanning
and identity-routing limitations. ANT admission waits through the startup
decision and acknowledgment-triggered acquisition; skipping the acknowledgment
for degraded running sensors does not itself disable ANT. No raw runtime power
command interface or general shutdown/sleep implementation is added.

The acknowledgment reaches the companion's existing sensor initialization.
Pressure coefficients are read into RAM. The motion branches configure sampling
and thresholds; QMA initialization uses soft reset and loads existing NVM into
working registers, without the documented NVM-program unlock. No settings-save,
calibration-programming or vendor-filesystem operation was found in the traced
startup path. This does not establish fitted-part identity or independently
measure calibration preservation. Existing firmware/dumps remain private.

N22 `0x15ba8` forwards button events 1, 2, 4 and 5 through `0x15b68`. That encoder
adds `8000` to the event and places the word at payload bytes 6–7, with button ID
at byte 1. This confirms the existing event-word decoding. It does not establish
press, hold, repeat or release timing. The three event-1 short-click mappings
remain the physical baseline in [companion.md](companion.md).

The same callback independently calls `0x16354`. Button 0 with event 4, or any
button event while a companion mode accessor returns 3, reconfigures the periodic
state-machine timer through `0x167c0` with argument 100. This is timer restart,
not evidence of a processor reset. Timer registration at `0x1a6ec` selects
callback `0x1a7b8`, which queues worker `0x1a68c` and its state dispatcher
`0x15f1e`. A main-MCU implementation cannot assume that ignoring a button report
prevents the companion from acting. Physical timing remains unverified. Do not
perform unattended holds to infer it.

| Stock operation | Main entry point | Recovered envelope and payload |
|---|---|---|
| Power off | `0x42052094` | Class 2, group `10`, `e2 02 00 00 00 00 00 00` |
| Power-on acknowledgment | `0x420520e8` | Class 2, group `10`, `e2 02 00 00 00 01 00 00` |
| Check power-on reason | `0x42052140` | Class 1, group `10`, `e2 02 00 00 00 01 00 00`; the receiver ignores the value byte |

These names come from stock callers. UART submission, a matching reply, USB
removal and a dark display do not establish electrical shutdown. No power-off or
sleep request was exercised for this research. A complete transition needs owned
storage durability, peripheral sequencing, loss of readiness, bounded completion
and a tested recovery path. Functional shutdown/wake tests require the owner
present if physical recovery may be needed.

### Shutdown receiver and completion limits

A further static trace on 2026-09-14 used the same N21 release 1.956 and N22
release 1.902 artifacts. N22 remains a supporting update image, not a readback
of the installed companion. No new device access accompanied this trace.

N22 receiver `0x178a4` accepts the fixed shutdown envelope above through
`0x178fa`, calling state setter `0x173a8(0)`. The setter clears the startup
acknowledgment and a pending button event, calls teardown `0x161b8`, enables
button-event handling through `0x15d1c(1)`, and enters state zero.

Teardown first calls `0x174dc`. This stops acquisition/service timers and sensor
bus activity, takes the selected motion-driver teardown branch, requests ANT
closure through `0x131fc(0)`, and disables companion GPIO controls at indices two
and three through `0x145c8`. It also drives companion GPIO23 and GPIO13 low.
The outer `0x161b8` then disables control indices one, five and zero, in that
order. Table `0x23b00` maps these five control indices to companion GPIO10,
GPIO11, GPIO12, GPIO7 and GPIO6 respectively. The control setter `0x1601c`
skips writes when its cached state already matches. These are companion pin
numbers, not ESP32 pins; rail names and their electrical effects remain
unverified. The traced teardown contains no identified flash erase,
vendor-filesystem write or calibration-program operation. That does not prove
all asynchronous radio work has completed.

Only after the setter returns does `0x17a14` construct the generic eight-byte
response `e2 02 01 00 00 00 00 00`. GPIO transitions precede this response, so
the main CPU may lose operation before receiving it. Other accepted setters
share this response shape. A correlated reply establishes receiver return,
not system power-off completion; no distinct shutdown-complete event was
identified. Missing replies, a dark panel and transport loss must retain
uncertain completion. A timeout must not trigger command replay.

State zero dispatches through `0x149b0`, which observes the companion's charging
input and consumes button events. The charging path `0x1613c` accepts top-left
event four and selects initialization state seven with subtype two. The other
path `0x161e0` has battery and button guards before selecting initialization.
The existing startup acknowledgment is a one-shot RAM flag, not an unconditional
remote wake command. Do not automatically send initialization value seven to
recover from a deliberate shutdown. If USB leaves the main CPU operating, keep
the transition uncertain until explicit recovery evidence arrives. Recovery
after a new boot must still establish fresh acquisition, as described above.

### Sleep remains a separate operation

Setter value three shares peripheral teardown but omits the outer shutdown
control sequence. Its state dispatch calls `0x14a4c`, consumes pending events
from all three buttons, and selects initialization state seven on a nonzero
event. This supports a companion suspend/resume candidate. It does not establish
a supported whole-device sleep operation, main-CPU low-power entry, USB wake or
electrical savings. N21's function named `MidSendSleepToLCD` at `0x4202ace4`
sets a software flag; that name alone supplies no panel command or timing
contract. Keep sleep and selectable wake sources unverified until their full
sequencing and physical recovery are established.

Battery decoding remains unchanged. N21 treats power-status zero as charging;
other values remain raw. Existing percentage and USB-transition observations do
not establish cell-voltage calibration, battery capacity or current entering the
battery. No replacement percentage curve or vendor calibration write is justified.

## ANT capability matrix

The installed baseline is documented in [ant.md](ant.md). A recovered N22 command
path is evidence about that update, not proof it executes on the installed unit.

| Capability | Current implementation / installed evidence | Recovered update behavior and remaining uncertainty |
|---|---|---|
| Receive role | Three concurrent selected receive channels, one peer per type; radar, heart-rate and standard power have physical evidence | Supported profile-type routing is recovered; this is not arbitrary raw ANT reception |
| Transmit | No public transmit operation | N22 has an additional group-to-radio-submit path described below; installed behavior and completion remain unverified |
| Acknowledged transfer | Not exposed | Radio service identification and peer acknowledgment propagation remain unresolved |
| Burst transfer | Not exposed | No complete bounded bridge burst path recovered; hardware support remains unknown |
| Channel assignment, period and RF | Not exposed | Companion owns type-to-channel lookup and configuration; no generic safe command recovered |
| Network configuration | Not exposed or changed | No persistent network configuration changes authorized; absence of a recovered API is not a hardware limit |
| Channel capacity | Implementation allows three and has three-sensor evidence | N22 type lookup includes more logical channel indices; this does not prove simultaneous capacity of the installed radio |
| Concurrent scan and receive | Current implementation requires closing channels | Installed coexistence remains unverified; retain the restriction |
| Multiple peers of one type | Prohibited | Data pages carry type without peer/channel identity; no stronger routing recovered |
| Identity | Startup/query report candidate exists | Fitted silicon and installed version remain unknown until corroborated |
| Suspend/resume | No verified power-transition integration | Coordinate tested channel teardown/recovery with device power sequencing |

N22 dispatch at `0x17b48` routes groups 11, 17, 34, 35, 40, 120, 121, 122, 123 and
128 through `0x13640`, then `0x135cc` and `0x16f0c`. The last function resolves a
logical channel by device type through `0x1529c`. A missing mapping returns
silently. Otherwise it issues radio supervisor call `c8`, with channel, payload
length and payload pointer. The precise service name requires a matching radio
stack ABI; this document does not infer acknowledged delivery from its number.

The response wrapper discards the radio call's result. It constructs an eight-byte
response containing the first two request payload bytes, a byte equal to 1 at
offset 2, and zeros in the remaining fields. Consequently, this positive bridge
reply cannot establish radio acceptance or peer delivery, even if it matches a
request. A future operation needs a separate verified completion observation,
bounded payload and queue limits, timeout/uncertain-completion semantics, and an
owned peer. Do not retry based on this reply alone. No new transmit request was
sent during this investigation.

The existing truncated identity acknowledgment and type-only data routing still
apply. The companion's larger type table neither restores missing device-number
bits nor permits independent routing of same-type peers.

## Bounded sound control and observations

N22 routes class 2, group 16, eight-byte payload `e2 01 PP 00 00 00 00 00`
to its fixed-pattern buzzer player. Bytes after the pattern selector do not
provide arbitrary frequency, duration or amplitude. The recovered PWM uses fixed
50% duty. No separate speaker or codec is established. Pattern 19 cancels playback;
it changes volatile playback state, without a configuration save in that path.
The bridge response does not establish acoustic completion.

The C606 capability exposes only patterns 0, 10, 21 and 22. Their recovered table
values are respectively 3 kHz/75 ms, 4 kHz/100 ms, 2.5 kHz/150 ms and 3 kHz/300 ms.
These durations are nominal timer values from the analyzed update, not measured
electrical or acoustic durations on the installed companion. The capability has
one pending request, rejects overlap, expires an unsent start after one second,
uses conservative settling guards, and requires explicit stop after uncertain
submission. Stop cancels a queued start. It never queues repetitions or retries
an uncertain start automatically. Pattern 18 has an unbounded counter-wrap path;
24 and 25 write companion configuration. None is accepted.

Base revision `740b36c83e76` produced all four tones through the actual C606.
The existing laptop microphone recorded them without a host speaker fixture.
Frequency-selective analysis found approximately 118, 356, 153 and 341 ms of
corresponding tonal energy, using 5 ms windows and 1 ms steps. Pattern 10's
extended 4 kHz event remains unexplained: the recording does not resolve separate
bursts or distinguish installed firmware, acoustic decay and host processing.
Do not claim its nominal 100 ms acoustic duration was verified. A second pattern
22 request followed by STOP shortened recorded tonal activity to roughly 190 ms.
Threshold choice affects endpoints; capture startup latency and absolute SPL are
unmeasured. Microphone gain/routing and device placement were retained.

Across this scenario companion reports advanced by 411 and valid GNSS sentences
by 252, with no new input, UART, CRC, DMA or parser errors. Battery/power reports
remained fresh. No active ANT peer was part of this observation. No shutdown or
sleep transition was tested. A main-CPU restart does not prove that companion
playback was cancelled; finite patterns remain the bound. Future power sequencing
must quiesce sound explicitly and verify its own recovery behavior. Issue #84
therefore retains its timing, ANT-coexistence and power-transition limitations.

## Installed read-only storage evidence

Base revision `740b36c83e76` completed the bounded MMC layout and clock scenario.
The FAT32 superfloppy spans all 7,733,248 reported sectors. Four distinct sector
reads reached the root directory terminator; child trees and allocation ownership
remain unverified. Repeated selected-sector CRCs matched at 400 kHz, 4 MHz and
20 MHz, with no new GNSS or companion errors observed during that scenario.
Representative repeated sector-zero read times were 10,934, 1,506 and 670
microseconds. These are bounded read samples, not sustained throughput results.
The original clock and preferences were restored. See [storage.md](storage.md)
for geometry and validation limits. No part of this vendor volume has established
application write ownership, and it is not a writable bulk-storage backend.

## Physical validation still needed

The morning procedure should use the reviewed build's ordinary command help and
private logger, with one USB owner. Save logs before changing USB or power state.

1. Record stationary pressure and both raw motion subtypes for a minute. Rotate
   the device through six stable faces, pausing about five seconds at each, then
   perform separate slow rotations. Record orientation and report subtype. This
   can establish observable axes and candidate scaling, not absolute accuracy.
2. Capture isolated clicks on each button, then owner-supervised holds with
   duration and release noted. Test combinations only after individual behavior
   and a recovery method are established. Software injection cannot substitute
   for physical input.
3. Capture USB disconnected/reconnected and a charge/discharge interval while
   preserving preferences and rides. Correlate raw power status and percentage;
   do not relabel USB presence as measured charging current.
4. Observe pressure during an elevation change and GNSS during a short outdoor
   session. A plausible change validates functional response, not independent
   pressure calibration. Keep coordinates and raw readings private.
5. Run reviewed shutdown/wake tests only with the owner ready to recover the
   device. Observe display, USB and startup separately, then confirm acquisition
   and owned storage after recovery. Current-consumption measurements remain
   deferred under the owner's equipment constraint.

Installed part identification, physical motion/hold/wake observations, independent
voltage/pressure calibration and new ANT operation end-to-end evidence remain open.

## Charging initialization and explicit wake

The recovered N22 class-2/group-16 receiver accepts page `e2/02`, operation index
zero, value seven as its normal initialization transition. Receiver `0x178a4`
reaches `0x173a8(7)` through `0x178fa`; setter seven changes RAM state, without the
shutdown branches used by values zero and three. State seven dispatches through
`0x1751e`. Retained boot subtype zero or two runs the same ordinary initialization
subtree and emits reason six or five. Subtype one performs partial initialization
without that reason; other retained values can complete without acquisition. This
is receiver-supported behavior, not a recovered N21 sender constant or a promise
of success for every retained state.

The device uses the fixed payload `e2 02 00 00 00 07 00 00` only once after an
explicit charging reason four and an explicit wake request, with a fresh, clean
bridge and no observed sensor or radio activity. A short/failed submission is uncertain and is never replayed.
Readiness requires a subsequent normal-start reason and independently advancing
fresh motion and pressure timestamps; timeout is a failure. The audited branches
contain no identified flash erase, vendor-filesystem write or calibration-program
path; the existing stock motion initialization retains its opaque internal limits.

Source `382c85cadeb1` kept probing when battery/power reports were absent. This
was insufficient for battery startup, which waits for the acknowledgment before
periodic reports and shuts down after its acknowledgment timeout. Source
`271749ae53ac` removes that prerequisite for the one-shot acknowledgment after
three seconds of clean passive observation. The charging initialization transition
still requires a fresh bridge. Live validation of source `382c85cadeb1` reached ready with reason
six and independently advancing sensors after charging startup, with zero button
reports after boot. This observation does not establish every power-cycle path.

### Main screen suspension is not verified CPU sleep

A further bounded trace of N21 1.956 follows `MidSendSleepToLCD` at
`0x4202ace4` through getter `0x4202ad54` and `0x42165664`. The touch reader at
`0x4202add0` uses the flag to suppress touch polling. Button routing at
`0x420531c5` consumes event one to invoke `ScreenOffWakeUpHandle` at
`0x42165a88`. These are screen/input mechanisms.

Screen-off dispatch at `0x420eb668` dims the backlight, sets that flag, calls
empty function `0x4229df40`, and conditionally invokes panel reinitialization at
`0x4202acfc`. Its companion payload uses control index three/value two, not
power-state value three. The corresponding wake-side function `0x4229df48` is
also empty. This traced path establishes neither ESP CPU sleep nor a UART/GPIO
wake configuration.

The companion power-state value-three constant exists in N21 at `0x3c375f89`,
but this analysis did not recover a sender call chain. Receiver support alone
remains insufficient to expose whole-device sleep. The class-one reason getter
instead references the shared value-one payload at `0x3c375f81`; the table above
corrects its previously reported value-three byte. The receiver's class-one
branch reads the reason without interpreting that byte.

### Shutdown and charging standby observations on 2026-09-14

The first coordinated shutdown build, `811c8749f46f`, accepted one shutdown request,
entered preparation and lost USB. The owner observed the screen turn off and then
return automatically. The next attachment reported a power reset and normal
acquisition. The existing automatic value-seven path made charging startup return
to normal operation; a focused startup regression reproduced that unwanted
request before the fix.

Source `84e49e0ad61c` requires an explicit wake request for value seven and retains
physical operating-reason recovery. A subsequent shutdown on USB returned in
`Charging` with normal work disabled. It remained there through uptime 47,929 ms;
charging preparation had completed at 6,324 ms. The owner confirmed that the
screen remained off. An explicit wake request at 47,975 ms reported completed
recovery at 49,026 ms. Pressure and both motion streams were fresh, GNSS was
receiving, Wi-Fi was associated and BLE was advertising. The owner confirmed
that the screen returned with that request.

This is functional shutdown-to-charging-standby and wake evidence. It does not
establish electrical power-off, current savings or whole-device sleep. Charging
presentation belongs to the shell; the device capability reports the charging
state without adding an animation. Source `1fbfd1f` additionally handles physical
wake during charging preparation and keeps failed charging preparation from
silently resuming radios. Raw captures remain private under `.local/power/`.

Final base/harness firmware `a7248d6f7ba2`, with code identical to `1fbfd1f`,
completed another USB shutdown-to-charging cycle. Saving the current settings
succeeded before the request; a save submitted after shutdown acceptance returned
`BUSY`. The new boot reached charging standby, and the owner confirmed standby
followed by normal screen return after a two-second top-left hold. Recovery
reported completion with operating reason five, fresh pressure and both motion
streams, GNSS reception, verified Wi-Fi connectivity and BLE advertising. The
four saved display/time preferences matched after the power transition, and a
bounded read of MMC sector zero succeeded. No synthetic button events supplied
this evidence. The device was left in normal operation on that base/harness
firmware, with no collector running.

Host tests cover admission during active IO, refused new writes, ambiguous
submission, transition/recovery deadlines, sound-stop recovery, ANT close
completion, physical wake during charging preparation and charging preparation
failure without an unintended restart. All four base/SDK and harness build
combinations passed. Battery-only shutdown, broader wake-source coverage,
whole-device sleep and electrical current measurements remain unverified in this
session. Charging standby and radio quiescence are not electrical-off claims.
