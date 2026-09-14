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

N22 `0x15ba8` forwards button events 1, 2, 4 and 5 through `0x15b68`. That encoder
adds `8000` to the event and places the word at payload bytes 6–7, with button ID
at byte 1. This confirms the existing event-word decoding. It does not establish
press, hold, repeat or release timing. The three event-1 short-click mappings
remain the physical baseline in [companion.md](companion.md).

The same callback independently calls `0x16354`. Button 0 with event 4, or any
button event while a companion mode accessor returns 3, enters a restart-timer
path through `0x167c0` with argument 100. A main-MCU implementation cannot assume
that ignoring a button report prevents the companion from acting. The timeout's
physical effect and units remain unverified. Do not perform unattended holds to
infer them.

| Stock operation | Main entry point | Recovered envelope and payload |
|---|---|---|
| Power off | `0x42052094` | Class 2, group `10`, `e2 02 00 00 00 00 00 00` |
| Power on | `0x420520e8` | Class 2, group `10`, `e2 02 00 00 00 01 00 00` |
| Check power-on reason | `0x42052140` | Class 1, group `10`, `e2 02 00 00 00 03 00 00` |

These names come from stock callers. UART submission, a matching reply, USB
removal and a dark display do not establish electrical shutdown. No power-off or
sleep request was exercised for this research. A complete transition needs owned
storage durability, peripheral sequencing, loss of readiness, bounded completion
and a tested recovery path. Functional shutdown/wake tests require the owner
present if physical recovery may be needed.

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
