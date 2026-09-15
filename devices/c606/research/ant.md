# ANT+ companion support

The [2026-09-15 investigation](ant-investigation/README.md) adds the observed
companion version, recovered 12-channel configuration, separate discovery
channels and outgoing-message findings. It distinguishes those findings from
the current four-channel implementation and verified hardware behavior.

Custom firmware uses the installed companion's sensor bridge. It does not replace
companion firmware or configure an ANT network key. The stock N21 1.956 wrappers
establish the protocol below; installed companion behavior is validated separately.
Exact companion chip and installed firmware version remain unverified.

## Scope and ownership

Device code translates the C606 companion protocol. Core owns bounded discovery,
up to four explicitly selected receive channels, one per device type, lifecycle and packet delivery. The cycling
SDK interprets radar pages. There is no radar-specific channel setup in core.

The bridge forwards a device type and eight data bytes, without a device number
or channel index. This implementation therefore selects up to four sensors of different device types.
It cannot prove the sender of a data page independently of the matching connection
report. Additional profile decoders can consume `ant::Packet` through the same
queue. The SDK composition is its sole consumer when enabled; the base console
can drain it with `ANT READ`. Queue overflow drops the oldest packet and stamps
loss on subsequent packets. Acquisition continues without a USB host.

Raw ANT channel assignment, RF/period configuration, network keys, transmit roles,
acknowledged writes and burst transfers are not exposed by this increment. It
implements the installed bridge's discovery and receive capabilities. Supported
profile types are constrained by companion firmware, not by a universal ANT radio
API. Stock routes types 11, 17, 34, 35, 40, 120, 121, 122, 123 and 128.

## Recovered wire format

All requests below use the existing companion envelope and CRC16/XMODEM.
Requests have class 2, reports class 4. Payloads are eight bytes. Multi-byte
numbers are little endian.

| Operation | Group | Payload |
|---|---|---|
| Scan | 16 | `e1 02 00 duration_lo duration_hi ff 00 00` |
| Stop scan | 16 | `e1 02 00 00 00 ff 00 00` |
| Connect | 1 | `17 type number_lo number_hi transmission 00 timeout_lo timeout_hi` |
| Disconnect | 1 | `17 type 00 00 00 01 00 00` |
| Discovery | 16 | `f3 03 type number_lo number_hi transmission rssi status` |
| Link event | 1 | `17 type number_lo number_hi transmission event ...` |
| Sensor page | device type | Eight unchanged data bytes |

The companion truncates the high device-number byte to zero in link events.
N22 code at `0x16d02` and `0x1bed2` shifts left before an eight-bit store; discovery
at `0x1c1ba` shifts right correctly. Live SR mini discovery and connection reports
confirmed this difference. The C606 adapter restores the high byte from the sole
selected identity only when device type, low byte and transmission type match,
and the reported high byte is zero. Core still compares full identities. This
correlates an acknowledgment to the request; the bridge cannot independently
confirm all sixteen identity bits in that acknowledgment.

Discovery status 0 signals scan end, 1–15 reports a sensor and other values are
ignored. RSSI is signed. Link events 3, 4 and 5 represent connected, disconnected
and timeout. Scan seconds are inferred from stock's duration × 1000 comparison
against its 50 ms processing ticks. Hardware timing must distinguish that inference
from the behavior of the installed companion.

Recovered entry points: scan `0x421ab44c`, stop `0x421ab4cc`, connect `0x421ab53c`,
disconnect `0x421ab660`, scan/link handling `0x421ab084`, data routing `0x4204bea8`,
and serialization `0x4204b000`. Vendor disassembly remains private in `.local/`.

## Console

Use `mise run terminal` with `CYCLING_PORT` set to the current USB port when needed.
Commands use decimal identity fields from discovery. Example identities below are
synthetic.

```text
ANT SCAN 10
ANT
ANT DEVICES
ANT STOP
ANT CONNECT 40 1234 5
ANT
ANT READ
ANT DISCONNECT 40
```

`ACCEPTED` means queued. Inspect `ANT` for completion; successful UART submission
alone does not prove the companion accepted a command. A failed submission is
uncertain and is not automatically replayed. Explicit reconnection uses
`ANT CONNECT` without restarting the C606. Discovery is bounded to eight identities
and scanning to 1–60 seconds. Keep terminal outputs containing identities private.

With the cycling SDK enabled, use `RADAR` to inspect interpreted reports. Type 40
is the radar profile. The decoder retains eight wire slots, not stable vehicle
identities. Distances are integer millimetres and closing speeds centimetres per
second. Each four-target page expires independently after two seconds; background
pages cannot keep target reports fresh. Errors and link/transport loss invalidate
targets. Unavailable or reserved threat states must not be presented as all clear.

The decoder is original code based on the Garmin Bike Radar Profile revision 2.1
field definitions, corroborated by stock's packed-track decoder at `0x421b15b0`.
The profile document is linked from the source and is not redistributed. ANT+
Adopter/licensing status has not been established for this project; this work does
not claim certification or complete device-profile compliance.

## Hardware evidence

The validation device is the owner's iGPSPORT SR mini. Its identifier and raw
radio/USB captures stay private. Commit-specific hardware results and remaining
acceptance gaps are recorded in the implementation PR for #73.

## Historical outdoor captures

Earlier SDK builds recorded raw ANT pages, link transitions and optional position
and environment samples into unused tail slots of the owned ride journal. That
on-device diagnostic recorder and its separate screen have been removed. VANA
uses the normal workout recorder; raw radar packets are not stored in new rides.

Historical capture slots remain occupied data. The ride scanner and full raw
export preserve them, including records without a terminal stop marker. To decode
a previously exported prefix without connecting the device:

```sh
mise run ant-export -- .local/ant-decoded --input .local/ride-export/ride-slots.bin
```

The offline decoder preserves the input range, checks capture commit/CRC records,
reports invalid slots and sequence gaps, and writes a SHA-256 manifest. Raw
identities and positions stay in ignored `.local/`. Use `mise run ride-export`
for a read-only download of the current journal before offline decoding.

## Multiple sensor types

The core keeps four independent channel lifecycles and packet queues, with at
most one selected peer for each device type. Packet draining rotates between
channels. A disconnect or command failure for one type does not reset the others;
a shared UART/CRC loss invalidates all channels. Scanning requires closing live
channels first. Use `ANT CHANNEL type` for detailed state and `ANT DISCONNECT type`
for a specific channel.

The SDK interprets radar type 40, heart-rate type 120 common fields, and bicycle
power type 11 standard power page 0x10. `RADAR SENSORS` reports fresh interpreted
heart-rate/power observations; raw capture retains all received pages, including
pages without a decoder. The test screen has separate RADAR, HEART and POWER
indicators. OFF means unselected; WAIT means selected without fresh channel data.
LOG SAVING still requires recent verified storage commits, independently of those
connection indicators. Starting a test requires every selected channel to be fresh.

The capture preserves sensor identity on each packet. New kind-5 link records
include device type; the exporter retains compatibility with earlier untyped
kind-4 link records. The capture buffers sixteen packets plus an eight-packet
pending batch. The 2,400-slot preflight is an allowance for a ten-minute
multi-sensor test, not a guaranteed duration at arbitrary packet rates. Full
storage stops capture without overwriting occupied records.


## GPS in the outdoor capture

The SDK samples the existing positioning snapshot once per second into the same
append-only capture. Kind-6 records carry sample uptime, the last valid fix
observation uptime when available, and paired latitude/longitude in degrees times
10^7 only while the core reports a fresh fix. Missing or stale fixes produce a
record without coordinates; they do not repeat a last known point as current.
GPS and ANT uptime timestamps share the same boot clock for later comparison.
The screen shows GPS FIX or WAIT separately from the number of verified saved
GPS records, which includes samples without a fix. The exporter keeps coordinates
and raw captures private. The preflight includes 600 extra slots for ten minutes
of GPS samples. Actual capacity still depends on sensor traffic and flush cadence.


`ANT STOP` retains scan ownership until the companion reports scan completion or
a two-second stop timeout expires. Another scan or connection cannot start during
that interval; duplicate stops are rejected. The bridge does not tag scan epochs,
so an acknowledgment arriving after the bounded timeout cannot be attributed to
an older scan with certainty.
