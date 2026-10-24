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
| `flake.nix`, `nix/` | Development environment and tool versions |

Keep package interfaces small and internals private. Split packages by
meaningful ownership, not file count.

## Checks

Use mise tasks and commit `Cargo.lock`. The flake dev shell provides every
tool; [dev-setup](docs/dev-setup.md) sets up a machine with one command.

- Inner loop: targeted host tests and the simulator; firmware rebuilds take
  about 20 s once sccache is warm.
- Before opening or updating a PR: `mise run verify --pr <n>` (all gates in
  parallel, results posted on the PR).
- Disk: every worktree carries about 4.5 GB of `.local` build targets.
  `mise run clean` prints what it would free; `--apply` removes worktrees whose
  branch is merged, clean and without a process inside them, and build dirs
  idle over `--idle-hours` (24). Verify and sim-compare warn under 30 GB free.

- Firmware changes: `mise run test`, `mise run check` (includes boundary checks
  and ruff) and `mise run build`. Host tasks cover base and cycling builds.
- Feature-boundary changes: also build base and cycling with both harness modes
  (`CYCLING_SDK=0|1`, `CYCLING_HARNESS=0|1`).
- PRs are verified locally, not by GitHub CI: run test, check, simulate,
  harness-virtual and all four builds, and post the results on the PR. CI runs
  only on pushes to main.
- Preserve the licensed [Trouble Host patch](vendor/trouble-host/CYCLING-PATCH.md)
  and its minimum-MTU (ATT MTU 23) regression coverage.

## Device safety

Read the [C606 skill](.agents/skills/c606/SKILL.md) before flashing, serial
access or device tests. It owns the procedure; these rules always apply:

- Change device firmware only through `mise run backup`, `mise run flash`,
  `mise run rollback`, `mise run stock`, `mise run stock-select` and
  `mise run restore-owned`.
- Stock lives in the verified private backup, not on the device (owner
  decision, 2026-10-17): cycling may occupy both A/B application slots, and
  `mise run stock` restores the complete backup. Never touch eFuses. Preserve
  the bootloader and partition table, and persisted cycling settings and rides
  across flashes. No vendor filesystem writes outside that full restore.
- Serialize all device access. Linked worktrees share the primary checkout's
  `.local` device lock and backup. When `CYCLING_DEVICE_HOST` is set the device
  is on that machine: reach it only through `mise run device <task>` and the
  [remote device skill](.agents/skills/remote-device/SKILL.md).
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
