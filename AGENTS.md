# cycling

Code and tools own implementation and verification mechanics; GitHub issues and
PRs own requirements, decisions and history. Read
[architecture](docs/architecture.md) before changing ownership or interfaces.

## Where to look

| Path | Owns |
| --- | --- |
| `apps/vana` | Cycling behavior and screens |
| `devices/c606` | Hardware and firmware composition; sanitized research in `research/` |
| `packages/device-api` | Capability contracts |
| `packages/shell` | Shared UI policy |
| `packages/services` | Portable mechanisms |
| `packages/console` | Device command sessions |
| `tools/simulator` | Host simulation |
| `tools/devtools` | Host device workflows |

Keep package interfaces small and internals private. Split packages by
meaningful ownership, not file count.

## Checks

Use mise tasks and commit `Cargo.lock`.

- Firmware changes: `mise run test`, `mise run check` (includes boundary checks
  and ruff) and `mise run build`. Host tasks cover base and cycling builds.
- Feature-boundary changes: also build base and cycling with both harness modes
  (`CYCLING_SDK=0|1`, `CYCLING_HARNESS=0|1`), as CI does.
- Preserve the licensed [Trouble Host patch](vendor/trouble-host/CYCLING-PATCH.md)
  and its minimum-MTU (ATT MTU 23) regression coverage.

## Device safety

Read the [C606 skill](.agents/skills/c606/SKILL.md) before flashing, serial
access or device tests. It owns the procedure; these rules always apply:

- Change device firmware only through `mise run backup`, `mise run flash` and
  `mise run stock`.
- Preserve stock ota_0, bootloader, partition table, eFuses and existing
  persisted data. No vendor filesystem writes.
- Serialize all device access.
- Validate changed hardware access or display timing on hardware, and report
  what was actually observed.
- Restore a harness-enabled base on the development device after tests.
- Publish code, tools and sanitized findings. Keep vendor firmware, disassembly,
  flash dumps, credentials, identifiers and raw captures in ignored `.local/`.

## Workflows

- Autonomous issue or queue delivery: [deliver](.agents/skills/deliver/SKILL.md).
- Issue tracker: GitHub Issues; read `docs/agents/issue-tracker.md` first.
- Triage: the five default labels; read `docs/agents/triage-labels.md` first.
- Domain docs: single-context layout; read `docs/agents/domain.md` before
  exploring domain concepts or proposing domain changes.
