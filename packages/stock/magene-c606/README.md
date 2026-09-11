# Magene C606

Research from stock release 1.956 and one physical C606. Written findings and our
analysis tools are public; vendor firmware, disassembly and device dumps are not.

- [Hardware map](hardware.md)
- [Companion buttons and battery](companion.md)
- [Wi-Fi bring-up](wifi.md)
- [Firmware format and boot process](firmware.md)
- [Partition layout](partitions.csv)
- [Unpacker](unpack.py)

Independent C firmware has booted through the original bootloader, driven the screen
and returned to stock. The Rust coin demo also boots through that bootloader: the user confirmed correct
colors and smooth rotation on 2026-09-08. USB reported completed frames every
42 ms, with render/transfer work taking 28–33 ms. The application image is 113,088
bytes; it uses no heap. Stock ota_0 remains verified and preserved.

## Local analysis

Supply your own firmware update file:

```sh
mise exec -- python packages/stock/magene-c606/unpack.py /path/to/N21_update.bin --output .local/stock
```

Output is local analysis material, not redistributable project source. The parser
reconstructs images and ELF load segments; it does not recover original symbols or
source code. It does not flash hardware.

On 2026-09-11, Rust touch input and a 5–100% brightness slider were flashed through
slot B. USB captured touch drags and PWM updates; the user confirmed accurate
finger tracking and visible brightness changes. The touch UI takes about 19 ms
per frame for input, rendering and transfer, at a 42 ms cadence. The application
image is 124,480 bytes with no heap. Stock slot A, bootloader and partition table
passed the device workflow verification. See the [bring-up notes](hardware.md#touch-and-brightness-bring-up-2026-09-11).

The same day's companion bring-up added battery percentage, reported voltage,
power status and all three physical buttons. The user confirmed the counters,
brightness shortcuts and USB unplug/replug status changes work. This application
is 136,112 bytes. See [companion protocol and validation](companion.md).
