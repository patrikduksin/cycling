# cycling

Open-source Rust firmware for the Magene C606 bike computer.

The base `no_std` firmware provides positioning, physical input, battery/power
observations, display submission, persistent settings, idle dimming, Wi-Fi,
network time and BLE transport. An ordinary USB terminal and bounded JSON logs
work with the test harness disabled. There is no graphical application or demo.

The optional `cycling` SDK adds one explicitly selected Heart Rate or Cadence
sensor, live ride recording, recovery, history and raw/JSON/GPX export. Live speed
and distance and simultaneous separate sensors are unsupported. Receiver identity
and measured GNSS accuracy remain follow-ups; MMC identification and reads are
verified, but vendor filesystem writes are not authorized.

```sh
mise install
mise run setup
mise run test
mise run check
mise run build                       # Base, development harness enabled
CYCLING_SDK=1 mise run build          # Add the cycling SDK
```

Use the repository's safe flash tasks. Stock slot A, the bootloader, partition
table, eFuses and existing persisted data must be preserved.

- [Development and build matrix](docs/development.md)
- [Safe device workflow](docs/device.md)
- [Terminal and device validation](docs/device-debugging.md)
- [Ride recording and export](docs/ride-recording.md)
- [Architecture and migration inventory](docs/core-architecture.md)
- [Development evidence and open work](docs/overnight-plan.md)
- [C606 hardware research](packages/stock/magene-c606)

Firmware lives in `packages/os`; device research lives in `packages/stock`.
Vendor images, device identifiers, credentials and raw evidence stay in ignored
`.local/`. [MIT license](LICENSE).
