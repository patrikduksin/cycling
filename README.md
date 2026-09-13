# cycling

Open-source Rust firmware for the Magene C606 bike computer. The `no_std` base
provides positioning, physical input, display, settings, networking, BLE and ANT
transport through an ordinary USB terminal. The optional cycling SDK adds one
selected BLE Heart Rate or Cadence sensor, ride recording, recovery and export.
It also interprets ANT radar, heart-rate and power reports and provides a sensor
test screen and capture. A full graphical application and live speed/distance
are not implemented.

See [architecture and ownership](docs/architecture.md) for the device capability
and shell boundaries, and [#75](https://github.com/patrikduksin/cycling/issues/75)
for implementation tracking.

```sh
mise install
mise run setup
mise run test
mise run check
mise run simulate                    # Shared shell with deterministic host devices
mise run harness-virtual             # Shared command/input/capture/persistence scenarios
mise run harness -- run input-screen # Real C606 input and screenshots
mise run build                       # Base, development harness enabled
CYCLING_SDK=1 mise run build          # Add the cycling SDK
mise run terminal -- STATUS          # Query an already-running device
mise run boot-stock                  # Verify, select and boot preserved stock
```

Setup installs the pinned Xtensa toolchain; builds load its environment without
changing your shell. [Mise](mise.toml), Cargo configuration and CLI `--help` own
the current tool versions, task catalog and check matrix.

The same shell source consumes display, input, power and owned-storage contracts
on C606 and in the host simulator. The simulator uses a framebuffer and scripted
events to check presentation and input policy. It does not model physical DMA,
radio coexistence or power sequencing.

To add a device, select its compatible HAL/runtime, implement the capabilities
required by the shell, and keep wiring, drivers, memory constraints and task
startup in its device module. Compose those handles with the shared shell in a
device entry point. Add the composition to the mise/CI checks and validate its
hardware behavior. Shell code must not need device imports or board-name branches.

Wi-Fi is unconfigured by default. Before building, `mise run wifi-setup` can
copy the connected NetworkManager personal-network profile into ignored
`.local/wifi/`. Credentials are compiled into the image; keep configured firmware
images private as well as the configuration.

Before connecting or flashing, read the [C606 workflow](.agents/skills/c606/SKILL.md).
It preserves stock firmware and existing data. Firmware lives in `packages/os`;
vendor artifacts, credentials and raw evidence stay in ignored `.local/`.

- [Architecture and ownership](docs/architecture.md)
- [Testing harness and recipes](docs/testing-harness.md)
- [Ride recording and export](docs/ride-recording.md)
- [C606 hardware research](packages/stock/magene-c606)
- [Requirements and history](https://github.com/patrikduksin/cycling/issues)
- [Agent delivery workflow](.agents/skills/deliver/SKILL.md)

[MIT license](LICENSE).
