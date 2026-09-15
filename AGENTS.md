# cycling

Read [architecture](docs/architecture.md) when changing ownership or interfaces.
Code and tools own implementation and verification mechanics; GitHub issues and
PRs own requirements, decisions and history.

Look in `apps/vana` for cycling behavior and screens, `devices/c606` for hardware
and firmware composition, `packages/device-api` for capability contracts,
`packages/shell` for shared UI policy, `packages/services` for portable mechanisms,
and `packages/console` for device command sessions. Host simulation lives in
`tools/simulator`, host device workflows in `tools/devtools`, and sanitized C606
research in `devices/c606/research`. Keep package interfaces small and internals
private. Split packages by meaningful ownership, not file count.

Use mise tasks and commit Cargo.lock. For firmware changes run `mise run test`,
`mise run check` and `mise run build`; host tasks cover base and cycling builds.
For feature-boundary changes build base and cycling with both harness modes.
Preserve the licensed Trouble Host patch and its minimum-MTU regression coverage.

Read [c606](.agents/skills/c606/SKILL.md) before flashing, serial access or device tests.
Use `mise run backup`, `mise run flash` and `mise run stock` for device firmware changes.
Preserve stock ota_0, bootloader, partition table, eFuses and existing persisted data.
No vendor filesystem writes. Serialize all device access and validate changed
hardware access or display timing on hardware; report what was actually observed.
Restore a harness-enabled base on the development device after tests.
Publish code, tools and sanitized findings; keep vendor firmware, disassembly,
flash dumps, credentials, identifiers and raw captures in ignored `.local/`.

For autonomous issue or queue delivery, read [deliver](.agents/skills/deliver/SKILL.md).
