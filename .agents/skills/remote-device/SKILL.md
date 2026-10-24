---
name: remote-device
description: Flash, test, read logs from or debug crashes on the C606 when it is plugged into another machine (CYCLING_DEVICE_HOST), and decode backtraces against the image built here.
---

# Remote device

Read the [C606 skill](../c606/SKILL.md) first: its rules and the device-safety
rules in [AGENTS.md](../../../AGENTS.md) apply unchanged. This skill adds how to
reach the device from a machine without it. [dev-setup](../../../docs/dev-setup.md)
explains the mechanism.

## Simulator first

Most behavior needs no hardware. Reproduce in a host test, `mise run sim`
(report and screenshots in `.local/sim/latest`) or `mise run harness-virtual`
first; `mise run sim-compare --pr N` attaches before and after screens. Go to
the device for what the simulator cannot show: drivers, timing, radio, power,
storage and real sensors.

## Device tasks

`CYCLING_DEVICE_HOST` set means the device is elsewhere. Run every device task
as `mise run device <task> [args]`; never ssh to the host to run tools there by
hand.

| Goal | Command |
| --- | --- |
| Is the device there? | `mise run device device-port` |
| One command | `mise run device terminal STATUS` (or `INFO`) |
| Flash this checkout | `CYCLING_SDK=1 mise run device flash` (builds here, then the host's A/B flash) |
| Logs | `mise run device logs --seconds 30` |
| Base, persistence, crash tests | `mise run device e2e`, `persistence-test`, `crash-test` |
| Back to the previous slot | `mise run device rollback` |

- The host holds the one USB lock. `Another cycling device task is using USB`
  means another agent has the device: wait and retry, never work around it.
- Evidence (logs, reports, exports) comes back to the same `.local/` path the
  task printed. Read it here.
- `CYCLING_*` task settings travel with the command; `device flash` builds
  the `CYCLING_SDK`/`CYCLING_HARNESS` variant they name.
- `stock`, `stock-select`, `backup` and `restore-owned` need the owner's
  go-ahead in the task; ask rather than run them.
- After device tests, leave a harness-enabled cycling build running
  (`CYCLING_SDK=1 CYCLING_HARNESS=1 mise run device flash`), as the C606 skill
  requires, and report what you observed.

## Crashes

`STATUS` reports `reset=` and `crash=` after a panic; `CYCLING_RESET` in the
boot log says why the device restarted. A panic prints its message and
`Backtrace: 0x42…` on the USB console, so it is only seen if logs were being
captured: reproduce with `mise run device logs --seconds N` running.

Decode with `mise run backtrace <log>` (or pipe the text in). It uses
`.local/cycling.elf`, which `build.sh` keeps beside every image, so decode in
the checkout that built the image that crashed; rebuilding a different commit
replaces it. Post the decoded frames, not raw addresses, in the issue or PR.
