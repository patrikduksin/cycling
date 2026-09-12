# cycling

Code owns implementation; GitHub issues and PRs own requirements, decisions and history.
Keep firmware in `packages/os` and device research in `packages/stock/<device>`.
Device hides board wiring/HAL/quirks; core owns general capabilities; the optional
cycling SDK owns cycling interpretation, rides, selection and export semantics.
Read [architecture](docs/architecture.md) when changing those ownership boundaries.

Use mise tasks and commit Cargo.lock. For firmware changes run `mise run test`,
`mise run check` and `mise run build`; the host tasks check both base and cycling SDK.
For feature-boundary changes build base and SDK with both harness modes.
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
