# Remote USB bridge: design

A visitor plugs a device into Chrome on https://cycling.computer. A developer's
agent runs `cycling remote <code>` and gets that device as a local serial port,
so `cycling terminal`, `cycling logs` and esptool work unchanged. This is the
design the code implements. It came from a four-way design bakeoff; candidate 3
was the base, with the grafts named below.

## Constraints

- Most devtools open the port with `os.open` plus `termios`
  (`terminal/usb.py` `open_no_reset`), so the agent needs a pty. esptool needs
  DTR/RTS for `--before usb-reset`, which a pty cannot carry, so the agent also
  serves RFC 2217 on the same remote stream.
- The byte stream is fully transparent: JSON lines, `C6VQ` frames, SLIP.
- Every chip reset re-enumerates USB. The browser must reacquire the granted
  port with no gesture, and the agent's endpoints stay up across the gap.
- DTR/RTS timing cannot cross a ~100 ms link, so a reset is one semantic
  message that the browser runs locally from one shared table.
- No MAC or serial number reaches the server or the UI. The C606's USB serial
  string is its MAC.

## Shape

```
package.json  bun.lock  alchemy.run.ts  tsconfig.base.json  .oxlintrc*.json  .oxfmtrc.json
web/protocol   @cycling/protocol  Schema only, pure. The wire contract.
web/relay      @cycling/relay     Worker (admission) + Session Durable Object + pure policy.
web/site       @cycling/site      Foldkit SPA: Web Serial bridge, WebUSB describe-only.
web/agent      @cycling/agent     bun CLI: relay client, Remote, pty, RFC 2217, reset recognizer.
tools/devtools/src/cycling_devtools/device/browser_bridge.py   `cycling remote` launcher
```

Dependencies point one way, `protocol <- {relay, site, agent}`. An oxlint
`no-restricted-imports` rule bans leaf-to-leaf imports and deep protocol
imports; `alchemy.run.ts` is the only composition root. Every Effect package is
pinned through one bun catalog entry at `~4.1.2` (Foldkit's peer range).

## Wire protocol (web/protocol)

- One WebSocket per role: `/v1/browser/<code>` and `/v1/agent/<code>`. The path
  and the `cycling.v1` subprotocol carry the major version.
- Binary message: raw device bytes, no header. The sender's role gives the
  direction. At most 16 KiB per frame. Text message: one JSON control object
  from the sender's union, decoded strictly (`onExcessProperty: "error"`).
- One ordered socket per role, relayed in order, so a `PortChanged Lost` is
  never reordered against the bytes around it.
- Coalescing, both clients: flush at 4 KiB or 10 ms (`coalesceBytes`).
- Keepalive: clients send `ping`; the relay answers `pong` by auto-response
  without waking the Durable Object.
- Credentials ride in `Sec-WebSocket-Protocol`, because browsers cannot set
  headers: `cycling.v1, auth.<token>` for the agent and `cycling.v1, own.<key>`
  for the browser. The relay echoes only `cycling.v1`.
- Admission failures are delivered as accept-then-close with an application
  close code (`CloseCode`), because a browser cannot read the HTTP status of a
  refused upgrade.
- Messages. Browser: `DeviceAnnounced`, `PortChanged`, `ResetResult`,
  `LineCodingResult`, `Bye`. Agent: `Reset{id, kind}`, `SetLineCoding{id, baud}`.
  Relay: `Welcome{role, snapshot}`, `PeerPresence`. Each
  client decodes the union of what it may receive, so a wrong-direction
  message fails at the boundary.
- A session's end is its close code: `visitorDisconnected` 4000,
  `visitorGone` 4001, `expired` 4408, named after the `EndReason`.
  `closeMeaning(code)` is the one reading of every code for both clients:
  `Transient | Ended | Replaced | Refused | Remint`.
- `Device` is `serial{vendorId, productId}` or `usb{descriptor}`. No schema has
  a serial-number field, and strict decoding refuses one instead of dropping it.
- `RESET_PLANS` is the one table of `setSignals` steps and holds. The browser
  executes it. The agent's recognizer derives what it matches from the same
  table (`recognitionOf`).

## Session (web/relay)

- Codes are 8 Crockford characters (40 bits) minted by the browser, together
  with a 128-bit owner key. The code addresses one Durable Object and pairs two
  humans; it authorizes nothing. The first claim stores `sha256(ownerKey)`; a
  claimant with another key gets `codeInUse` and the browser mints a new code.
- The Worker checks the path, the code, the version, the browser's Origin and
  per-IP rate limit, and the agent token (constant-time compare), before the
  Durable Object is touched. Agents cannot create sessions.
- Durable state is one record (`Open | Paired`, owner-key hash, times, device,
  port), the alarm and the WebSocket tags. Presence is derived from the tagged
  sockets. Nothing lives in memory, so a hibernation wake needs no rehydration.
- A second socket of the same role closes the older one with `replaced`, so an
  agent restart or a page reload is idempotent.
- TTLs: open 15 min, paired sliding 2 h, hard cap 6 h, browser grace 5 min.
  Expiry is one idempotent alarm.
- The decision logic is a pure `decide` function; the Durable Object
  interprets its effects.

## Agent (web/agent)

```
RelayClient (socket, token, capped redial)
  -> Remote.attach(code): RemotePort     epochs, request ids, results, port up/down
       -> arbiter `route`: one active door
            -> pty door      Bun.Terminal, CYCLING_PORT = <dir>/<CODE>/port
            -> RFC 2217 door 127.0.0.1:<n>, CYCLING_ESPTOOL_PORT, ResetRecognizer
  control.sock: `reset <kind>` (the explicit fallback verb)
```

- Active door: device bytes go to the RFC 2217 client while one is connected,
  otherwise to the pty. Writes from the inactive door are dropped and counted.
- The pty is `Bun.Terminal` in-process. The bridge holds the slave open, so a
  tool closing the pty never hangs it up. The backlog toward the pty is a
  bounded dropping queue, so a pty with no reader never stalls the relay.
- While the port is down, writes are dropped and counted; a pty cannot report
  EIO on demand, and the tools already tolerate gaps.
- The reset recognizer is a pure state machine over the RFC 2217 client's
  SET-CONTROL commands. Its fixtures are recorded from real esptool.
- `cycling remote <code> -- <cmd>` runs one command with both variables set,
  mirrors its exit code and tears everything down.

## Site (web/site)

- One `Page` union: `Unsupported | Idle | Choosing | Live | Ended`, with `Relay`
  and `Port` sub-unions inside `Live`.
- Bytes never pass through `update`. Two ManagedResources (serial port, relay
  socket) are keyed on the Model, and an `attempt` counter in the key re-keys
  them for reacquire and redial. Two Subscriptions carry bytes: device to relay
  (needs both up) and relay to device plus control (needs the relay only).
  The monitor receives one `Traffic` message per 100 ms batch.
- `requestPort` is the first action of the click's Command (user activation).
- A reload resumes from `sessionStorage` with no gesture, because the port
  grant persists.

## Tradeoffs accepted

- A custom framing (raw binary plus JSON text) instead of base64 envelopes.
- No replay across a relay drop: bytes in flight are lost, and presence and
  `Lost` tell both sides. A stale queue written into a device after a reset is
  worse than a gap.
- A hand-written RFC 2217 subset instead of a dependency.
- The agent token is in `Sec-WebSocket-Protocol`, visible to Cloudflare like
  any header.
- The RFC 2217 port is loopback TCP; any local user can dial it.
- A 40-bit code; secrecy is the token and the owner key, and rate limits guard
  enumeration.

Rejected: a `socket://` leg (no modem lines, `--before usb-reset` silently does
nothing), a stateful relay with an in-memory peer map (breaks on hibernation),
base64 envelopes, bytes through `update`, two sockets per role (reordering).

## Implementation reconciliation

Where the code differs from the sketch, and why.

- **Reset table and recognizer.** The sketch read esp_pylib's `PIN_HIGH` as
  `true`; pyserial's convention is the opposite. `RESET_PLANS` now follows the
  levels real esptool 5.4.0 sends over RFC 2217 (fixtures in
  `agent/test/fixtures/`), including the zero-hold (true, true) level esptool
  walks through. `hardReset` was added for `--after hard-reset`, as an RTS-only
  plan. The recognizer reads SET-CONTROL commands, not deduplicated levels:
  pyserial's open sends DTR and RTS on, so a hard reset after a plain open
  starts with an RTS-on that changes no level. The first command per line only
  initializes it. (On the C606 an RTS hard reset can leave the chip in ROM
  download mode; devtools reboot with `watchdog-reset`, which needs no lines.)
- **Protocol.** The agent has no `Bye`: an agent leaving is a socket close and
  never ends the session. End reasons are `visitorDisconnected`, `visitorGone`
  (grace ran out) and `expired`. `superseded` became the `replaced` close code,
  for both roles. The `Ended{reason}` message is gone: the close code carries
  the reason, so there is one signal, not a message and a code that could
  disagree. Before, the relay never sent 4408 and the agent and the site read
  it differently; now an expired session closes with it.
- **One client.** Agent, site, relay smoke and e2e all dial through the
  protocol's `relaySocket`, on `effect/socket`. It checks the selected
  subprotocol, pings, and returns decoded frames ending in one `Closed`;
  redial stays with each client, on the shared `BACKOFF` curve.
- **Admission.** Refusals are accept-then-close with a close code, since a
  browser cannot see a refused upgrade's status. Alchemy's `upgrade` sets no
  `Sec-WebSocket-Protocol`; the relay sets `cycling.v1` on the 101 itself
  (checked on local workerd only). The Worker hashes the owner key and strips
  the subprotocol header, so the Durable Object never sees a credential.
  The relay Worker runs with observability and invocation logs off, because
  the credentials are request headers. `AGENT_TOKEN` must be at least 32
  characters; with a shorter one every agent is refused and the Worker logs
  why. Binary frames skip `decide`: the Durable Object checks the size and
  forwards them.
- **Relay.** Layout A: the relay is its own Worker on `relay.cycling.computer`
  and the site is on the apex. The record has `browserGoneAt` for the grace
  alarm. Only text frames renew the TTL, which keeps storage off the byte path.
  The rate limit is Cloudflare's binding (namespace 1001, 20 browser opens per
  minute per address, creates and resumes alike, failing open).
- **Stages.** Only `prod` gets custom domains and `Cloudflare.state()`; other
  stages use local state, so `alchemy dev` never touches the account.
  `relay/test/offline-smoke.sh` proves it by running the relay in a network
  namespace with only loopback and an empty `HOME`.
- **Agent pty.** `Bun.Terminal.write` buffers without bound and its `drain`
  only follows a buffered write, so device bytes go to the master fd with
  non-blocking writes behind a 32-chunk dropping queue. Bun does not expose
  the slave path; it is read from the fds the constructor opened, and opening
  fails loudly if that ever changes. No socat.
- **Agent sockets.** The relay socket is the protocol's `relaySocket`; the
  RFC 2217 server and the control socket are `@effect/platform-bun` socket
  servers, its `-- <cmd>` child is an Effect `ChildProcess`. The agent refuses
  a relay that does not select `cycling.v1`, a token shorter than 32
  characters, and `ws://` to anything but loopback. The child never sees the
  token. Extra exit codes: 5 port did not open in 30 s, 1 local resource
  failure, 127 the command did not start; a session that ends while waiting
  exits with the ending's code.
- **Paths.** The launcher (`device/browser_bridge.py`) passes
  `CYCLING_REMOTE_DIR` = the main checkout's `.local/bridge` (token at
  `.local/bridge/token`); a session lives
  in `<dir>/<CODE>/` (`port` symlink, `control.sock`). `EsptoolFlasher` gives
  esptool `CYCLING_ESPTOOL_PORT` only when its port is the bridge's
  `CYCLING_PORT`, so a second, local device in a bridged command is not
  redirected. Checks run inside
  `cycling check` and a `test-web` task in `cycling test`.
- **Site.** The serial port is read whenever it is open, even while the relay
  is down (bytes are dropped and counted), because Chrome's receive buffer
  overruns when nobody reads. `sessionStorage` also keeps baud and epoch, so a
  reload does not restart epochs. Bytes toward the port wait in a queue of 32
  frames written by their own fiber, so `Reset`, `SetLineCoding` and a close
  are handled while a write is stuck; a reset or close drops what is queued
  and counts it. On reacquire the page reopens the port object the visitor
  picked when it reappears, else the first unopened match. The monitor ring is 100 rows (150 overran
  Foldkit's patch budget). DevTools are off in production, because the Model
  holds the owner key.
- **Look.** The page follows VANA's rules (docs/ui.md): black is the layout,
  the session code is the hero, words are small capitals, and colour marks
  state only (green live, amber in between, red lost). Tokens are chrome.rs
  values; light mode runs them through `screens::look::light`'s formula.
  Nunito is self-hosted from `packages/shell/fonts` (SIL OFL). Every screen is
  one centred 38rem column on one spacing scale; the code scales with
  `clamp()` against the widest code so it never wraps; buttons are 48px with
  an accent focus ring; the byte monitor is a fixed-height scroll area.

Open risks, cheapest experiment first: user activation for `requestPort` in
real Chrome (passed in headless Chromium only); the subprotocol echo on the
real edge; the re-enumeration gap against the 25 s boot budget; whether a
port lost during a reset plan should count as success; the browser can send
`Lost` ahead of the last coalesced bytes; the agent's outbound queue toward
the relay is unbounded (R9).
