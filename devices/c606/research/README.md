# Magene C606

Research from stock release 1.956 and one physical C606. Written findings and our
analysis tools are public; vendor firmware, disassembly and device dumps stay in
ignored `.local/` (see [AGENTS.md](../../../AGENTS.md)).

- [Hardware and fitted-part uncertainty](hardware.md)
- [Companion wire protocol](companion.md)
- [Companion foundation research](foundation-research.md)
- [ANT+ companion support](ant.md) and its [investigation](ant-investigation/README.md)
- [Storage investigation](storage.md)
- [Vendor firmware format and boot process](firmware.md)
- [Partition layout](../partitions.csv)
- [Safe device workflow](../../../.agents/skills/c606/SKILL.md)

The read-only [unpacker](unpack.py) takes your own update file:

```sh
uv run --script devices/c606/research/unpack.py /path/to/N21_update.bin --output .local/stock
```

It reconstructs images and ELF load segments, not original symbols or source, and
never flashes hardware. Its output is private analysis material.

Historical evidence: [Wi-Fi bring-up](https://github.com/patrikduksin/cycling/issues/9#issuecomment-5647388326)
and [base/SDK validation](https://github.com/patrikduksin/cycling/pull/71#issuecomment-5647388096).
