# cycling

Open-source Rust firmware for the Magene C606 bike computer. The `no_std` base
provides positioning, physical input, display, settings, networking and BLE
transport through an ordinary USB terminal. The optional cycling SDK adds one
selected Heart Rate or Cadence sensor, ride recording, recovery and export.
There is no graphical application; live speed/distance and simultaneous separate
sensors are not implemented.

```sh
mise install
mise run setup
mise run test
mise run check
mise run build                       # Base, development harness enabled
CYCLING_SDK=1 mise run build          # Add the cycling SDK
mise run terminal -- STATUS          # Query an already-running device
```

Setup installs the pinned Xtensa toolchain; builds load its environment without
changing your shell. [Mise](mise.toml), Cargo configuration and CLI `--help` own
the current tool versions, task catalog and check matrix.

Wi-Fi is unconfigured by default. Before building, `mise run wifi-setup` can
copy the connected NetworkManager personal-network profile into ignored
`.local/wifi/`. Credentials are compiled into the image; keep configured firmware
images private as well as the configuration.

Before connecting or flashing, read the [C606 workflow](.agents/skills/c606/SKILL.md).
It preserves stock firmware and existing data. Firmware lives in `packages/os`;
vendor artifacts, credentials and raw evidence stay in ignored `.local/`.

- [Architecture and ownership](docs/architecture.md)
- [Ride recording and export](docs/ride-recording.md)
- [C606 hardware research](packages/stock/magene-c606)
- [Requirements and history](https://github.com/patrikduksin/cycling/issues)
- [Agent delivery workflow](.agents/skills/deliver/SKILL.md)

[MIT license](LICENSE).
