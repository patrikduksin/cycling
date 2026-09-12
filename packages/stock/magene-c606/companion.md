# Companion buttons and battery

Recovered from main stock release 1.956, with live C606 captures on 2026-09-11.
The companion update used for supporting analysis is N22 release 1.902; it has
not been read back from the installed companion. Captures and disassembly stay
in ignored `.local/`.

## Transport

Stock's logical UART 1 maps to ESP32-S3 UART2, TX42/RX41. Initialization at
`0x42009d74` supplies 115200 baud to the wrapper at `0x4203b1e8`.
The configuration is 8 data bits, no parity, one stop bit, no flow control.

Custom firmware receives on GPIO41 and sends the stock-derived GPS-open candidate
once on TX42. The stream predates that command; its control effect is unverified. The companion streams reports while our application runs, including
buttons, battery, power status, time and other sensor pages. This demonstrates
communication with the installed companion, not its exact chip or firmware ID.

## Frame format

| Offset | Meaning |
|---|---|
| 0 | `a5` sync |
| 1 | Payload length plus four |
| 2–3 | `6f f1` marker |
| 4 | Message class; observed companion reports use `04` |
| 5 | Report group |
| 6 onward | Payload |
| Final two bytes | CRC16/XMODEM, little-endian, over all preceding bytes |

Total frame length is byte 1 plus four. Eight-byte payloads produce sixteen-byte
frames. Stock framing and checksum checks are at `0x4204b140` and `0x4204b227`;
checksum entry points are `0x42218090` and `0x422a2a1c`.

The [decoder](../../os/src/companion.rs) owns validation and resynchronization.
The [input service](../../os/src/services/io.rs) owns freshness and loss reporting.

## Reports implemented

Offsets below are relative to the eight-byte payload, not the frame.

| Group / page | Fields | Evidence |
|---|---|---|
| `00 / 52` | Battery voltage at bytes 4–5, little-endian; percentage at byte 6 | Main handler `0x4204b78d`; periodic live reports |
| `10 / f0`, byte 1 = `02` | Power status at byte 2 | Main handler `0x4204bc7c`; stock charging branch at `0x42165b40` |
| `10 / 49` | Button ID at byte 1; event word at bytes 6–7, little-endian, offset by `0x8000` | Main handler `0x4204bbf9`; physical press sequences |

The voltage scale is interpreted as millivolts from companion battery conversion
and capacity-table analysis. N22's ADC conversion at `0x1cc44` uses a scale of
0.87890625 multiplied by four. Capacity-table endpoints are 3007 and 4295.
The reported value is not an independent measurement of cell voltage or calibration accuracy.
Percentage comes from the companion, not a new voltage-to-percentage estimate.

Stock treats power status zero as charging. USB power transitions were physically confirmed on 2026-09-11 in the retired UI.
Charging status alone does not prove current enters a full battery. Current core
code exposes the raw status; meanings beyond stock's zero remain unverified.

| Button ID | Position | Observed event |
|---|---|---|
| 0 | Top left | Short click, wire event `0x8001` |
| 1 | Bottom left | Short click, wire event `0x8001` |
| 2 | Bottom right | Short click, wire event `0x8001` |

Physical tests captured IDs 1 and 2, then three isolated ID 0 clicks. The user
confirmed all three counters and the former brightness shortcuts. Current core
code exposes button events without assigning navigation or brightness policy.
Long presses, other event words, combinations and power control remain unverified.

Host regression tests retain CRC, fragmented/corrupted-frame and resynchronization
coverage. Raw captures and physiological observations stay private.
