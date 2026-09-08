# Magene C606

Research from stock release 1.956 and one physical C606. Written findings and our
analysis tools are public; vendor firmware, disassembly and device dumps are not.

- [Hardware map](hardware.md)
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
