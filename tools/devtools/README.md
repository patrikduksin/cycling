# Device development tools

This Python package owns host access to firmware, private captures, and scenario
execution. Use the root `mise` tasks for routine operations. `mise` supplies the
source package path, so ordinary tools need no installation. Flash and backup
commands use the locked `uv` project to provide esptool. Bluetooth tools use the
system Python for the distribution's D-Bus and GLib bindings.

## Where behavior lives

- `device/` owns port discovery, protected flash operations, MMC inspection and
  maintenance. Flash safety checks stay beside the operation they protect.
- `terminal/` owns no-reset USB access, the device lock, command framing and logs.
- `rides/` owns export validation, decoding and explicit journal reclamation.
- `harness/` owns scenarios, real and virtual transports, camera and audio capture,
  and acquisition regression checks.
- `connectivity/` owns host Wi-Fi setup and Bluetooth test peers.
- `workspace.py` locates the checkout for commands and ignored private evidence.

Imports name the owning module. Package initializers do not re-export symbols.
Keep private outputs under the checkout's `.local/`; moving a tool must not change
its device lock or the location of existing backups and credentials.

## Run and test

From the repository root:

```sh
mise run terminal -- --help
mise run harness -- recipes
mise exec -- python -m cycling_devtools.harness.connectivity --help
mise exec -- python -m unittest discover -s tools/devtools/tests -p 'test_*.py'
```

The tests use fake transports and temporary files. They preserve coverage for
protected flash, uncertain writes, backup verification and ride export/reclaim.
Hardware scenarios need the device access rules in the C606 skill.

## ANT transport capture

`ANT CAPABILITIES` reports implemented limits and discovery state. `ANT CHANNEL
<type>` retains the existing snapshot text. `ANT READ` now drains an independent,
bounded diagnostic queue in both base and cycling builds. It cannot remove pages
from the application queue. Its packet `loss_count` includes diagnostic overflow;
selected identity is host correlation because the bridge omits sender identity.

`ANT SEND <type> <number> <transmission> <generation> <hex16>` sends exactly eight
bytes to the selected connection generation. `ACCEPTED operation=<id>` means
queued locally. Poll `ANT OPERATION <id>` for UART submission or bridge reply.
Neither establishes radio delivery or sensor execution. Only the most recent
operation observation is retained. There is no automatic retry.

Connect owned peers explicitly before a capture, then run:

```sh
mise exec -- python -m cycling_devtools.connectivity.ant \
  --port /dev/ttyACM0 --output .local/ant-check-001 \
  --types 120 121 --seconds 10 --scan-seconds 5 \
  --fixture-notes 'Record board revision, companion version, build modes, sensor categories and active traffic'
```

The tool uses the existing exclusive USB lock, captures baseline, discovery and
post-stop windows, and calculates per-peer counter rates and loss deltas. It
records source revision, dirty state, packet timestamps, generations and final
observations. Rate windows reflect USB polling boundaries; inspect discovery
state observations before attributing a rate change to scanning. Counts establish
only the fixtures actually observed.

An optional `--send 'type number transmission generation hex16'` issues one
explicit request after those windows. Use a harmless information request on an
owned sensor and establish response attribution separately from periodic pages.
The tool leaves delivery unknown and never retries a timed-out send. It does not
connect peers, change preferences, recover transports, or perform sleep/wake.
Those hardware checks remain explicit operator steps under the C606 skill.

All captures and reports stay under `.local/`, including peer identifiers and
raw page payloads. Publish only manually sanitized findings. Missing fixtures or
unperformed coexistence/recovery checks remain open hardware acceptance items.
