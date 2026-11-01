# Contributing

Thanks for being here. Every kind of contribution counts, and you don't need
to write code to make this project better. If you're unsure where to start,
say hi on [Discord](https://discord.gg/tQdv2QTJR5) or email
[patrikduksin@gmail.com](mailto:patrikduksin@gmail.com). I answer everyone.

## Ways to help

- **Report a bug.** Something wrong on the screen, a sensor that won't pair,
  a ride that saved oddly. [Open a bug report](https://github.com/patrikduksin/cycling/issues/new?template=bug.yml).
  A photo of the screen helps a lot.
- **Ask for a feature.** Tell me what you miss on your bike computer, or what
  your old one did better. [Request a feature](https://github.com/patrikduksin/cycling/issues/new?template=feature.yml).
  Small ideas are welcome.
- **Ask for your device.** [Open a device request](https://github.com/patrikduksin/cycling/issues/new?template=new-device.yml)
  with the make and model. If you're up for it, photos of the board and the
  output of plugging it in over USB help us judge whether it can be flashed.
- **Ride and give feedback.** Rough edges you notice on real rides are the
  most valuable thing anyone can send.
- **Write code or docs.** Pick any issue, or one labelled
  [good first issue](https://github.com/patrikduksin/cycling/labels/good%20first%20issue),
  and send a pull request.

AI agents are welcome contributors too. Open issues and pull requests the same
way people do. [AGENTS.md](AGENTS.md) has the rules agents follow in this repo.

## Pull requests

1. For anything bigger than a small fix, open or comment on an issue first so
   we can agree on the idea. This saves you from building something twice.
2. Keep each pull request to one change.
3. Run the checks below and paste the result into the pull request.
4. Say what you tested on a real device, if you have one. "Tested in the
   simulator only" is fine, just say so.

Don't worry about getting everything right. I'll help you get a pull request
over the line.

## Building and testing

Any x86_64 or aarch64 Linux machine with [Nix](https://nixos.org/download)
works. The repository provides every other tool.

```sh
git clone https://github.com/patrikduksin/cycling && cd cycling
nix develop                         # dev shell with every pinned tool
cycling test
cycling check
cycling simulate                    # run the firmware on your computer
cycling build                       # base firmware
CYCLING_SDK=1 cycling build         # with the VANA cycling app
cycling verify                      # every check in parallel
```

`cycling` alone lists every task. You don't need a bike computer to
contribute: the simulator runs the screens and input on your computer.

Before you connect to or flash a device, read the
[C606 guide](.agents/skills/c606/SKILL.md). Always take the backup first, and
only change device firmware through the `cycling` tasks.

## How the code is laid out

| Path | What it owns |
| --- | --- |
| `apps/vana` | Cycling features and screens |
| `devices/c606` | Magene C606 hardware support and research notes |
| `packages/device-api` | What a device must provide |
| `packages/shell` | Shared UI policy |
| `packages/services` | Portable building blocks |
| `tools/simulator` | Runs the firmware on your computer |
| `tools/devtools` | The `cycling` command |

More detail lives in [docs/architecture.md](docs/architecture.md).

## Adding a device

New devices are the most exciting contributions this project gets. Each one
lives in `devices/<device>`:

1. Pick a HAL and runtime that fits the chip.
2. Implement the capabilities in `packages/device-api` that the shell needs:
   display, input, positioning, radios, storage. Keep wiring, drivers, memory
   limits and task startup inside the device folder.
3. Compose those pieces with the shared shell in a device entry point, and add
   it to the `cycling` tasks and checks.
4. Validate it on real hardware and write down what you observed.

Shared code never imports device code or branches on a board name. Start by
opening a device request issue, even if you plan to do all of it yourself.
Figuring out how to flash a device safely is often the hardest part, and it
helps to plan it together.

## Documentation

- [Development machines and remote devices](docs/dev-setup.md)
- [Architecture and repository map](docs/architecture.md)
- [Testing harness](docs/testing-harness.md)
- [Screens and UI](docs/ui.md) and [ride flows](docs/ux-flows.md)
- [Runtime connectivity](docs/runtime-connectivity.md)
- [Ride recording and export](docs/ride-recording.md)
- [MMC volumes: rides, files and terrain](docs/mmc-volumes.md)
- [C606 hardware research](devices/c606/research)

## Be kind

We're all here because we like riding bikes. Be patient and friendly with
each other, especially with people who are new to firmware or to open source.
