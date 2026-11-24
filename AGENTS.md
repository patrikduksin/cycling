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
| `web` | cycling.computer: relay, site and the `cycling remote` USB bridge ([README](web/README.md)) |
| `flake.nix`, `nix/` | Development environment and tool versions |

Keep package interfaces small and internals private. Split packages by
meaningful ownership, not file count.

## Checks

Run tasks through `cycling <task>` and commit `Cargo.lock`. The flake dev
shell provides every tool and the `cycling` command; see
[dev-setup](docs/dev-setup.md).

- Inner loop: run `cycling quick` after every edit.
  `cycling build` after a VANA edit takes about 45 s, most of it the fat-LTO
  link, and about 2 s after a commit that changes no firmware input.
  [Checks you can run on your own work](docs/dev-setup.md#checks-you-can-run-on-your-own-work)
  lists the deeper ones and when each is worth its time.
- Verify, review and merge with the [verify skill](.agents/skills/verify/SKILL.md),
  before you open or update a PR. GitHub CI runs only on pushes to main, and
  `cycling merge <n>` is the only way to merge.
- Firmware changes: `cycling test`, `cycling check` (includes boundary checks
  and ruff) and `cycling build`.
- Disk: a worktree that ran `cycling verify` carries about 7 GB of target dirs
  (`target/`, `vendor/trouble-host/target/`, `.local/firmware`). Run
  `cycling clean` for a dry-run table of what it would free; `--apply` removes
  merged, clean, quiet worktrees nobody is working in, and idle build dirs.
  Verify and sim-compare warn when the disk runs low.
- Preserve the licensed [Trouble Host patch](vendor/trouble-host/CYCLING-PATCH.md)
  and its minimum-MTU (ATT MTU 23) regression coverage.

## Device safety

Read the [C606 skill](.agents/skills/c606/SKILL.md) before flashing, serial
access or device tests. It owns the procedure; these rules always apply:

- Change device firmware only through `cycling backup`, `cycling flash`,
  `cycling flash-maintenance`, `cycling rollback`, `cycling stock` and
  `cycling stock-select`.
- Stock lives in the verified private backup, not on the device: cycling may
  occupy both A/B application slots, and `cycling stock` restores the complete
  backup. Never touch eFuses. Preserve
  the bootloader and partition table. No vendor filesystem writes outside that
  full restore.
- A flash keeps settings and rides while their formats are unchanged:
  `cycling flash` compares the stub's MD5 of the owned reservations before and
  after the write. Each persisted format has one version byte. A format change
  bumps it and wipes the data. Firmware refuses data with another version as
  foreign and never overwrites it.
- Serialize all device access. Linked worktrees share the primary checkout's
  `.local` device lock and backup. When `CYCLING_DEVICE_HOST` is set the device
  is on that machine: reach it only through `cycling device <task>` and the
  [remote device skill](.agents/skills/remote-device/SKILL.md).
- Validate changed hardware access or display timing on hardware, and report
  what was actually observed.
- Leave cycling firmware running on the development device after tests.
- Publish code, tools and sanitized findings. Keep vendor firmware, disassembly,
  flash dumps, credentials, identifiers and raw captures in ignored `.local/`.

## Workflows

- A cycling.computer connect code, or a new device to check for cycling
  firmware: [new-device](.agents/skills/new-device/SKILL.md).
- Autonomous issue or queue delivery: [deliver](.agents/skills/deliver/SKILL.md).
- A rider's account of a ride problem, a battery that drains too fast, or a
  `.trace` file: [ride-trace](.agents/skills/ride-trace/SKILL.md).

## Spending tokens

Every tool call sends the whole conversation again, so a session costs its
number of calls times its size.

- Batch lookups. Send related searches and reads as parallel calls in one
  message, or chain them in one command when one feeds the next. Do not spend
  a turn on each `grep`.
- Keep an agent to one slice: a unit a fresh agent finishes in about 100 tool
  calls. Past that, commit, and hand the rest to a fresh agent with the
  brief, the branch and what is left.
- A long hunt through code you will not edit goes to an explore agent. Keep
  its answer, not its output.
- An owner that only waits on its workers reports and returns. Wait on a
  build with a background command, not a `sleep` loop.
- Merge what other streams build on first: a shared kit, and deletions. Then
  fan out, four streams at most. A stream that needs an open PR stacks on its
  branch.
- New simulator scenarios go in a file per screen. `cycling check-boundaries`
  holds the large files at their size.
