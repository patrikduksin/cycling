# cycling

Open-source Rust firmware for the Magene C606 bike computer.

- **Base** (`no_std`): positioning, physical input, display, settings, networking,
  BLE and ANT transport, buzzer patterns, GNSS pause/resume, raw motion,
  compensated pressure and owned MMC maintenance, all through an ordinary USB terminal.
- **VANA** (optional, `CYCLING_SDK=1`): workout screens, four concurrent ANT
  sensors plus one selected BLE Heart Rate or Cadence sensor, proximity radar alerts, and
  ride recording, recovery and export. It shows wheel speed, power, heart rate,
  active time, approximate gradient and training zones.

## Development

```sh
nix develop                         # Nix dev shell with every pinned tool (docs/dev-setup.md)
cycling test
cycling check
cycling simulate                    # Shared shell with deterministic host devices
cycling harness-virtual             # Shared command/input/capture/persistence scenarios
cycling build                       # Base, harness enabled
CYCLING_SDK=1 cycling build         # Add VANA
cycling verify                      # Every PR gate in parallel
```

`cycling` lists every task. [flake.nix](flake.nix) owns tool versions;
[cli.py](tools/devtools/src/cycling_devtools/cli.py), Cargo configuration and
CLI `--help` own tasks, options and the check matrix. `nix develop` enters the
dev shell; `nix develop -c <command>` runs one command in it.

Before connecting to or flashing a device, read the
[C606 skill](.agents/skills/c606/SKILL.md). Device tasks include
`cycling terminal STATUS`, `cycling harness run input-screen` and
`cycling stock`.

Wi-Fi is unconfigured until provisioned over USB; firmware images contain no
credentials. `cycling wifi-setup` copies the current NetworkManager profile into
ignored `.local/wifi/`; see [runtime connectivity](docs/runtime-connectivity.md).

## Adding a device

Select a compatible HAL/runtime and implement the capabilities the shell needs in
`devices/<device>`, keeping wiring, drivers, memory constraints and task startup
there. Compose those handles with the shared shell in a device entry point, add
the composition to the `cycling` tasks and CI checks, and validate it on hardware. Shell
code must not need device imports or board-name branches. The host simulator
checks presentation and input policy, not DMA, radio coexistence or power sequencing.

## Documentation

- [Agent and contributor rules](AGENTS.md)
- [Development machines and remote devices](docs/dev-setup.md)
- [Architecture and repository map](docs/architecture.md)
- [Testing harness](docs/testing-harness.md)
- [Runtime connectivity](docs/runtime-connectivity.md)
- [Ride recording and export](docs/ride-recording.md)
- [MMC volumes: rides, files and terrain](docs/mmc-volumes.md)
- [C606 hardware research](devices/c606/research)
- [Agent delivery workflow](.agents/skills/deliver/SKILL.md)
- [Requirements and history](https://github.com/patrikduksin/cycling/issues)

[MIT license](LICENSE).
