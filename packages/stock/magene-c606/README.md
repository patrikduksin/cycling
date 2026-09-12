# Magene C606

Research from stock release 1.956 and one physical C606. Written findings and our
analysis tools are public; vendor firmware, disassembly and device dumps stay private.

- [Hardware and fitted-part uncertainty](hardware.md)
- [Companion wire protocol](companion.md)
- [Storage investigation](storage.md)
- [Vendor firmware format and boot process](firmware.md)
- [Stock partition layout](partitions.csv)
- [Safe device workflow](../../../.agents/skills/c606/SKILL.md)

Supply your own update file to the read-only [unpacker](unpack.py):

```sh
mise exec -- python packages/stock/magene-c606/unpack.py /path/to/N21_update.bin --output .local/stock
```

It reconstructs images and ELF load segments, not original symbols or source.
Output is private analysis material. It does not flash hardware.

Historical evidence lives in [Wi-Fi bring-up](https://github.com/patrikduksin/cycling/issues/9#issuecomment-5647388326)
and [base/SDK validation](https://github.com/patrikduksin/cycling/pull/71#issuecomment-5647388096).
