# Device development tools

This Python package owns host access to firmware, private captures and scenario
execution. Use the root mise tasks (`mise tasks` lists them). Tools run in this
package's locked uv environment (`uv run --project tools/devtools --locked`),
which also provides esptool, pytest and ruff. The Bluetooth tasks
(`bluetooth-echo`, `ble-simulator`) instead use system Python for the
distribution's D-Bus and GLib bindings, with the source tree on `PYTHONPATH`.

Device access follows the [C606 skill](../../.agents/skills/c606/SKILL.md);
[device/port.py](src/cycling_devtools/device/port.py) implements its port selection.

## Where behavior lives

| Module | Owns |
| --- | --- |
| `device/` | Port discovery, protected flash operations, MMC inspection and maintenance. Flash safety checks stay beside the operation they protect. |
| `terminal/` | No-reset USB access, the device lock, command framing and logs |
| `rides/` | Export validation, decoding and explicit journal reclamation |
| `harness/` | Scenarios, real and virtual transports, camera and audio capture, acquisition regression checks |
| `connectivity/` | Host Wi-Fi setup and Bluetooth test peers |
| `clean.py` | `mise run clean`: the dry-run plan and removal of merged worktrees and idle build dirs, and the low-disk warning verify and sim-compare print |
| `workspace.py` | Checkout location and the rule that private evidence stays inside ignored `.local/` |

Imports name the owning module; package initializers do not re-export symbols.
Moving a tool must not change its device lock or the location of existing
backups and credentials.

## Run and test

```sh
mise run terminal -- --help
mise run harness -- recipes
mise run test     # includes pytest
mise run check    # includes ruff format --check and ruff check
mise run fmt      # applies ruff format
```

Modules without a task run as
`mise exec -- uv run --project tools/devtools --locked python -m cycling_devtools.<module>`.
Tests use fake transports and temporary files and keep coverage for protected
flash, uncertain writes, backup verification and ride export/reclaim.

## ANT transport capture

| Device command | Behavior |
| --- | --- |
| `ANT CAPABILITIES` | Implemented limits and discovery state |
| `ANT CHANNEL <type>` | Existing snapshot text |
| `ANT READ` | Drains an independent, bounded diagnostic queue in base and cycling builds; cannot remove pages from the application queue. `loss_count` includes diagnostic overflow. |
| `ANT SEND <type> <number> <transmission> <generation> <hex16>` | Sends exactly eight bytes to the selected connection generation. `ACCEPTED operation=<id>` means queued locally. No automatic retry. |
| `ANT OPERATION <id>` | UART submission or bridge reply for the most recent operation only |

Connect owned peers explicitly, then capture:

```sh
mise exec -- uv run --project tools/devtools --locked python -m cycling_devtools.connectivity.ant \
  --output .local/ant-check-001 \
  --types 120 121 --seconds 10 --scan-seconds 5 \
  --fixture-notes 'Record board revision, companion version, build modes, sensor categories and active traffic'
```

- The tool holds the USB lock, captures baseline, discovery and post-stop windows,
  and computes per-peer counter rates and loss deltas. It records source revision,
  dirty state, packet timestamps, generations and final observations.
- If the local window has already ended, it skips STOP and records physical
  completion as unknown. A rejected STOP is accepted only when readback shows the
  window inactive.
- `--send 'type number transmission generation hex16'` issues one request after
  the windows. Use a harmless information request on an owned sensor. Delivery
  stays unknown, and a timed-out send is never retried.
- It does not connect peers, change preferences, recover transports or perform
  sleep/wake; those remain explicit operator steps under the C606 skill.
- Captures and reports, including peer identifiers and raw payloads, stay under
  `.local/`.

What this does not prove:

- `ACCEPTED` and operation results do not establish radio delivery or sensor execution.
- Selected identity in `ANT READ` is host correlation; the bridge omits sender identity.
- Rate windows follow USB polling boundaries; check discovery state before
  attributing a rate change to scanning.
- Counts cover only the fixtures actually observed. Response attribution must be
  established separately from periodic pages. Missing fixtures and unperformed
  coexistence or recovery checks remain open hardware acceptance items.
