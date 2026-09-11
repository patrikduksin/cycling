# Companion buttons and battery

Recovered from main stock release 1.956, with live C606 captures on 2026-09-11.
The companion update used for supporting analysis is N22 release 1.902; it has
not been read back from the installed companion. Captures and disassembly stay
in ignored `.local/`.

## Transport

Stock's logical UART 1 maps to ESP32-S3 UART2, TX42/RX41. Initialization at
`0x42009d74` supplies 115200 baud to the wrapper at `0x4203b1e8`.
The configuration is 8 data bits, no parity, one stop bit, no flow control.

Custom firmware receives on GPIO41 only. It does not configure TX42 or send any
commands. The companion streams reports while our application runs, including
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

The Rust decoder validates marker, length and CRC before decoding an event. It
handles split and adjacent frames, skips unrelated report groups, and
resynchronizes after malformed frames. UART errors or a receive gap over 250 ms
clear partial data. Battery and power displays expire independently after five
seconds without their respective reports.

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
The custom UI displays the reported value in volts to two decimal places. This
is not an independent measurement of cell voltage or calibration accuracy.
Percentage comes from the companion, not a new voltage-to-percentage estimate.

Stock treats power status zero as charging. The first custom boot reported
100 percent, 4337 mV and status zero while USB was connected. USB connected or
charging status does not by itself prove that current is entering a full battery.
The UI labels zero `CHARGING`, one `ON BATTERY`, and other values `POWER UNKNOWN`.
The user confirmed the display changed correctly when USB was unplugged and
reconnected, and the device continued running on battery.

| Button ID | Position | Observed event |
|---|---|---|
| 0 | Top left | Short click, wire event `0x8001` |
| 1 | Bottom left | Short click, wire event `0x8001` |
| 2 | Bottom right | Short click, wire event `0x8001` |

The initial ordered test captured IDs 1 and 2. A separate top-left-only test
captured three ID 0 clicks. Firmware counts valid button events and highlights
the last button. Short clicks on bottom left/right adjust brightness by five
percentage points, clamped to 5–100. Top-left clicks update its counter.
Other nonzero button event codes are counted and logged without assigning a
long-press or release meaning or changing brightness. No shutdown, reset or
button-combination actions are implemented.

## Validation

Host tests cover a standard CRC vector, fragmented reports, corrupted frames,
resynchronization, invalid values, unrelated pages and partial-buffer reset.
Initial live captures contained checksum-valid periodic battery/power reports
and physical button events. The parser-enabled application booted from slot B
and reported battery and power state over USB. The user confirmed all three
button counters and brightness shortcuts work, and battery status changes
correctly through USB unplug/replug. The application image is 136,112 bytes.
Stock slot A is verified by the flash workflow; the
bootloader, partition table and eFuses are preserved.

A post-reconnection USB capture reached 340 valid frames with zero CRC failures
and zero UART errors. Long presses and button combinations remain unverified.
