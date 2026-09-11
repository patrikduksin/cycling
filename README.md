# cycling

Open-source bike computer firmware, written in Rust. Starting with the Magene C606.

A small `no_std` firmware with touch, buttons, battery status, brightness control, Wi-Fi, a pixel-art
Rust coin renderer, and the hardware notes needed to build the rest.

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
