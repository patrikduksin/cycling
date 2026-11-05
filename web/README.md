# web

cycling.computer and the `cycling remote` USB bridge. A visitor plugs a device
into Chrome or Edge at https://cycling.computer and reads a code to a
developer. The developer's agent runs `cycling remote <code>` and gets that
device as a local serial port for console tools and as a flasher for the
firmware tasks, so `cycling terminal`, `cycling logs`, `cycling backup` and
`cycling flash` run as if it were plugged in here. The page runs the chip's
ROM conversation itself (esptool-js), one relay round trip per device verb.
The design and its decisions are in [DESIGN.md](DESIGN.md).

| Path                | Owns                                                                             |
| ------------------- | -------------------------------------------------------------------------------- |
| `protocol`          | The wire contract: codes, keys, messages, device ops, frame codecs               |
| `relay`             | Cloudflare Worker and Session Durable Object at relay.cycling.computer           |
| `site`              | The visitor page (Foldkit): Web Serial bridge, update mode, WebUSB describe-only |
| `flasher`           | esptool-js as Effect, and the ROM simulator its tests and the e2e run against    |
| `agent`             | The bridge `cycling remote` runs: pty, device socket, lease flasher              |
| `../alchemy.run.ts` | The one Alchemy stack that deploys relay and site                                |

## Use a visitor's device

```console
$ cycling remote 7KQM-4XP2
$ cycling remote 7KQM-4XP2 -- cycling terminal INFO
$ cycling remote 7KQM-4XP2 -- cycling backup
$ cycling remote 7KQM-4XP2 -- cycling flash
$ cycling remote 7KQM-4XP2 -- cycling flasher info
```

- Without `--`, the bridge prints `CYCLING_PORT` (a pty) and
  `CYCLING_FLASHER` (the device socket) and stays up until the visitor
  disconnects. Export both in another shell and use any device task.
- With `-- <command>`, it waits for the visitor's port to open, runs the
  command with both variables set, mirrors its exit code and tears down. This
  is the form for agents.
- The firmware tasks (`cycling backup`, `flash`, `rollback`, `stock` and the
  rest) reach the chip through `CYCLING_FLASHER` when their port is the
  bridged `CYCLING_PORT`: devtools' `PageFlasher` asks the agent, the agent
  asks the page, and the page runs the op in esptool-js next to the chip. The
  page shows the visitor that their device is in update mode, what is being
  read or written, and asks once per session before the first write. Only
  Device's verbs exist (enter, read, write, verify, reboot); arbitrary
  `python -m esptool` commands do not work through the page. None of this has
  run against a real device yet: the first hardware session needs Patrik
  present.
- The page takes any chip family esptool-js detects (ESP8266, ESP32 and every
  ESP32-S, -C, -H, -P and -E part it names), on its own USB or behind a
  USB-UART bridge (CP210x, CH340, FTDI), and reboots it the way python esptool
  does for that family and port.
- `cycling flasher info|read ADDR SIZE PATH|verify ADDR PATH|reboot` runs the
  same verbs on any such chip, here (esptool, `--chip auto`, `--port` for a
  device other than the C606) or through the page. `info` prints the chip as
  JSON with an opaque `device_id` (a keyed hash) in place of the MAC; `read` writes only under
  `.local`; there is no write verb. Each verb ends with a reboot into the app.
- A stranger's device cannot be backed up or flashed by the `cycling` flows
  yet: `.local/device` holds one device's private state. Per-device state is
  its own follow-up.
- Exit codes: 0 the session ended, 2 bad code or token, 3 relay unreachable,
  4 unsupported protocol version, 5 the visitor's port did not open; in `--`
  mode, the command's own code.
- The token is read from `.local/bridge/token` in the main checkout (or
  `CYCLING_REMOTE_TOKEN`). Session files live in `.local/bridge/<CODE>/` and
  are removed on exit. `CYCLING_REMOTE_RELAY` overrides the relay URL.

Device tasks still take the main checkout's `.local/usb.lock` when they open
`CYCLING_PORT`, so a remote task and a local one never run at once.

## Develop

Every tool comes from the dev shell (`nix develop`). From the repo root:

```console
bun install --frozen-lockfile
bun run check                                  # typecheck, both lints, format check, tests
sh web/relay/test/offline-smoke.sh             # relay under `alchemy dev`, no network, smoke test
sh web/relay/test/offline-smoke.sh web/agent/test/e2e-relay.ts   # cycling remote end to end, fake browser
sh web/agent/test/e2e-page.sh [shots-dir]      # the page flasher end to end: Chromium, ROM simulator, Device
E2E_BENCH_MIB=4 sh web/agent/test/e2e-page.sh  # time one 4 MiB read through all of it at the C606's 140 KB/s
bun web/flasher/test/read-bench.ts [MiB] [KB/s] # time one read through the flasher alone; 0 KB/s: no chip limit
VITE_RELAY_URL=ws://localhost:1337 bun run --cwd web/site dev    # the page; ?fake shows a fake device
```

`cycling check` and `cycling test` run the same checks and tests. Stages other
than `prod` keep Alchemy state on local disk under `.alchemy/` and get no
custom domain, so `alchemy dev` touches nothing on Cloudflare.

## Deploy

Deploying needs the Cloudflare profile in `~/.alchemy` and an agent token. The
first `prod` plan or deploy bootstraps Alchemy's state store on the account
(a Worker and a Secrets Store), once.

```console
mkdir -p .local/bridge && openssl rand -hex 32 > .local/bridge/token && chmod 600 .local/bridge/token
AGENT_TOKEN=$(cat .local/bridge/token) bun alchemy plan --stage prod
AGENT_TOKEN=$(cat .local/bridge/token) bun alchemy deploy --stage prod --yes
```

Run these in the main checkout, whose `.local/bridge/token` every worktree
reads. The token must be at least 32 characters (`rand -hex 32` gives 64);
with a shorter one the relay refuses every agent and the agent refuses to
start. An existing `.local/remote/token` moves to `.local/bridge/token`.
