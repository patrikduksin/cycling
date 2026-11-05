# web

cycling.computer and the `cycling remote` USB bridge. A visitor plugs a device
into Chrome or Edge at https://cycling.computer and reads a code to a
developer. The developer's agent runs `cycling remote <code>` and gets that
device as a local serial port, so `cycling terminal`, `cycling logs` and
esptool work as if it were plugged in here. The design and its decisions are
in [DESIGN.md](DESIGN.md).

| Path                | Owns                                                                     |
| ------------------- | ------------------------------------------------------------------------ |
| `protocol`          | The wire contract: codes, keys, messages, frame codecs, the reset table  |
| `relay`             | Cloudflare Worker and Session Durable Object at relay.cycling.computer   |
| `site`              | The visitor page (Foldkit): Web Serial bridge, WebUSB describe-only      |
| `agent`             | The bridge `cycling remote` runs: pty, RFC 2217 server, reset recognizer |
| `../alchemy.run.ts` | The one Alchemy stack that deploys relay and site                        |

## Use a visitor's device

```console
$ cycling remote 7KQM-4XP2
$ cycling remote 7KQM-4XP2 -- cycling terminal INFO
$ cycling remote 7KQM-4XP2 -- sh -c 'python -m esptool --port "$CYCLING_ESPTOOL_PORT" --before usb-reset chip-id'
$ cycling remote reset 7KQM-4XP2 usbJtagBootloader
```

- Without `--`, the bridge prints `CYCLING_PORT` (a pty) and
  `CYCLING_ESPTOOL_PORT` (an `rfc2217://` URL) and stays up until the visitor
  disconnects. Export both in another shell and use any device task.
- With `-- <command>`, it waits for the visitor's port to open, runs the
  command with both variables set, mirrors its exit code and tears down. This
  is the form for agents.
- The esptool tasks (`cycling flash`, `backup` and the rest) pass
  `CYCLING_ESPTOOL_PORT` to esptool when it is set. esptool's
  `--before usb-reset` is recognized on the RFC 2217 side and runs in the
  visitor's browser as one reset. None of this has run against a real device
  yet: the first hardware session needs Patrik present.
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
