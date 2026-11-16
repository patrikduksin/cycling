# C606 firmware

The `c606-firmware` package owns the Magene C606 hardware implementation and the
firmware assembled for it. Its executable retains the name `cycling-os` so build
artifacts and flashing workflows keep a stable name.

- `src/main.rs` composes device handles, the shell, console and VANA runtime.
- `src/board.rs` allocates hardware resources and starts device tasks.
- `src/capabilities/` implements the shared device contracts.
- `src/drivers/` owns component protocols, HAL access and hardware quirks.
- `tests/` checks host-testable device behavior.
- `research/` contains sanitized hardware and stock firmware findings.

Hardware modules depend on device contracts and portable services. The composition
binary can depend on applications; that does not make application imports suitable
inside drivers. Preserve DMA ownership, interrupt behavior, memory placement and
power sequencing when changing hardware code.

[device.toml](device.toml) declares the C606 once: its build target and
variants, partitions, owned flash ranges, panel and buttons. `cycling devices`
checks it and writes [src/fitted.rs](src/fitted.rs), the constants the drivers
read; `cycling check` fails while that file is stale, and
`cycling devices --check` compares the attached unit's `FITTED` reply with
the manifest. [devices/build.sh](../build.sh) (run by `cycling build`) builds
the firmware, or with `--variant maintenance` (`cycling build-maintenance`) the
MMC maintenance image (see [storage research](research/storage.md)), and writes
`.local/cycling.bin` for either application slot; `cycling flash` picks the
slot that is not selected. The partition table it generates beside the image
matches the device layout; never flash it or a generated bootloader.

Device access, flashing and hardware validation follow
[AGENTS.md](../../AGENTS.md) and the [C606 skill](../../.agents/skills/c606/SKILL.md).
Host tests do not establish physical timing or hardware behavior.
