# ANT+ companion support

Custom firmware uses the installed companion's sensor bridge. It does not replace
companion firmware or configure an ANT network key. The stock N21 1.956 wrappers
establish the protocol below; installed companion behavior is validated separately.
Exact companion chip and installed firmware version remain unverified.

## Scope and ownership

Device code translates the C606 companion protocol. Core owns bounded discovery,
one explicitly selected receive channel, lifecycle and packet delivery. The cycling
SDK interprets radar pages. There is no radar-specific channel setup in core.

The bridge forwards a device type and eight data bytes, without a device number
or channel index. This implementation therefore selects one sensor at a time.
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
ANT DISCONNECT
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
