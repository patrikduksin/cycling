# C606 firmware

The `c606-firmware` package owns the Magene C606 hardware implementation and the
firmware assembled for it. Its executable retains the name `cycling-os` so build
artifacts and flashing workflows keep a stable name.

- `src/main.rs` composes device handles, the shell, console and optional VANA runtime.
- `src/board.rs` allocates hardware resources and starts device tasks.
- `src/capabilities/` implements the shared device contracts.
- `src/drivers/` owns component protocols, HAL access and hardware quirks.
- `tests/` checks host-testable device behavior.
- `research/` contains sanitized hardware and stock firmware findings.

Hardware modules depend on device contracts and portable services. The composition
binary can depend on applications; that does not make application imports suitable
inside drivers. Preserve DMA ownership, interrupt behavior, memory placement and
power sequencing when changing hardware code.

`build.sh` (run by `mise run build`) selects features from `CYCLING_SDK`,
`CYCLING_HARNESS` and `CYCLING_BULK_MAINTENANCE` (the MMC maintenance image; see
[storage research](research/storage.md)) and writes `.local/cycling.bin`
for slot B (`ota_1`). `partitions.csv` matches the device layout; never flash it
or a generated bootloader.

Device access, flashing and hardware validation follow
[AGENTS.md](../../AGENTS.md) and the [C606 skill](../../.agents/skills/c606/SKILL.md).
Host tests do not establish physical timing or hardware behavior.
