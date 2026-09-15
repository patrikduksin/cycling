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

Use root mise tasks for builds and device access. Read the
[C606 workflow](../../.agents/skills/c606/SKILL.md) before opening serial ports,
flashing or testing hardware. Host tests do not establish physical timing or
hardware behavior; record only observations actually made on a device.

Keep vendor firmware, dumps, disassembly, identifiers and raw captures in ignored
`.local/` at the repository root. Sanitized research belongs here; vendor artifacts
do not. Preserve stock firmware and existing persisted data.
