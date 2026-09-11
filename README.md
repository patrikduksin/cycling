# cycling

Open-source bike computer firmware, written in Rust. Starting with the Magene C606.

The `no_std` firmware has touch and button controls, brightness, battery status,
persistent settings, idle dimming, Wi-Fi recovery and network time. It uses verified
2 MiB PSRAM and reserved flash regions for settings and rides.

Ride features include a deterministic demo, live recording with GPS/battery fields, recovery
after reset, recent ride history, and USB raw/JSON export with GPX for recorded
location tracks. Live speed and distance are still unavailable. BLE supports minimum-MTU
GATT echo testing and one explicitly selected continuous Heart Rate or Cadence sensor;
simultaneous sensors, GNSS outdoor/model validation and SD/MMC storage remain follow-ups.

[Ride recording and export](docs/ride-recording.md) ·
[Device testing tools](docs/device-debugging.md) ·
[Development progress](docs/overnight-plan.md)

The initial renderer demo:

<img src="docs/demo.gif" alt="A pixel-art Rust coin rotating on the C606 display" width="240">

```sh
mise install
mise run setup
mise run build
```

- [`packages/os`](packages/os) — firmware
- [`packages/stock/magene-c606`](packages/stock/magene-c606) — hardware research and analysis tools
- [`docs`](docs) — development and device testing

[Development](docs/development.md) · [Device workflow](docs/device.md) · [MIT license](LICENSE)
