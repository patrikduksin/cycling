# Remote USB bridge: design

A visitor plugs a device into Chrome on https://cycling.computer. A developer's
agent runs `cycling remote <code>` and gets that device as a local serial port
for console tools and as a flasher for the firmware tasks, so `cycling terminal`,
`cycling logs`, `cycling backup` and `cycling flash` run the same code they run
on a device plugged in here. This is the design the code implements. The bridge
came from a four-way design bakeoff (candidate 3 the base); the page flasher
(#345) from a second one (candidate 2 the base, with grafts from 1 and 3). The
reconciliation sections at the end say where the code differs from both.

## Constraints

- Most devtools open the port with `os.open` plus `termios`
  (`terminal/usb.py` `open_no_reset`), so the agent needs a pty.
- The ROM's and stub's timeouts were written for USB: the 8.4 s bootloader
  watchdog, the 0.2 s wait for the stub's `OHAI`, the 15 ms watchdog-reset
  reply. Python esptool over a 0.35 s relay round trip misses all three
  (#345), so the ROM conversation runs in the page, next to the chip, in
  esptool-js. One relay round trip per `Device` verb crosses the wire.
- The byte stream is fully transparent: JSON lines, `C6VQ` frames, SLIP.
- Every chip reset re-enumerates USB. The browser must reacquire the granted
  port with no gesture, and the agent's endpoints stay up across the gap.
- No MAC or serial number reaches the server, the UI or a log. The C606's USB
  serial string is its MAC. The one exception is the authenticated agent:
  `Entered{chip}` carries the eFuse MAC to it, because devtools checks backups
  against the device's identity. The relay forwards that frame and stores
  nothing of it; the page never puts it in its Model or view.

## Shape

```
package.json  bun.lock  alchemy.run.ts  tsconfig.base.json  .oxlintrc*.json  .oxfmtrc.json
web/protocol   @cycling/protocol  Schema only, pure. The wire contract.
web/relay      @cycling/relay     Worker (admission) + Session Durable Object + pure policy.
web/site       @cycling/site      Foldkit SPA: Web Serial bridge, the page's op runner, WebUSB describe-only.
web/flasher    @cycling/flasher   esptool-js as Effect: one ROM+stub session on a SerialPort. Knows no relay.
web/agent      @cycling/agent     bun CLI: relay client, Remote, pty, device socket, lease flasher.
tools/devtools/src/cycling_devtools/device/browser_bridge.py   `cycling remote` launcher
tools/devtools/src/cycling_devtools/device/page_flasher.py     PageFlasher: Device's verbs over the device socket
```

Dependencies point one way, `protocol <- {relay, site, agent}`, and
`flasher <- site`. An oxlint `no-restricted-imports` rule bans leaf-to-leaf
imports except site to flasher, bans `@cycling/flasher` from importing any
`@cycling/*` (so a public flasher page can use it without the relay), and bans
deep imports; `alchemy.run.ts` is the only composition root. Every Effect package is
pinned through one bun catalog entry at `~4.1.2` (Foldkit's peer range).

## Wire protocol (web/protocol)

- One WebSocket per role: `/v2/browser/<code>` and `/v2/agent/<code>`. The path
  and the `cycling.v2` subprotocol carry the major version; v1 clients are
  refused with `unsupportedVersion`.
- Binary message: a channel byte, then its payload, at most 16 KiB in all.
  `0x00 | device bytes` is the console (what the pty sees). `0x01 | id u32 LE |
  offset u32 LE | bytes` is one op's bulk data (a write payload from the agent,
  read data from the browser), in 15 KiB chunks from `opDataFrames`. A frame's
  meaning never depends on a mode the two ends learn at different times. The
  relay checks only the size. Text message: one JSON control object from the
  sender's union, decoded strictly (`onExcessProperty: "error"`).
- One ordered socket per role, relayed in order, so a `PortChanged Lost` is
  never reordered against the bytes around it.
- Coalescing, both clients: flush at 4 KiB or 10 ms (`coalesceBytes`).
- Keepalive: clients send `ping`; the relay answers `pong` by auto-response
  without waking the Durable Object.
- Credentials ride in `Sec-WebSocket-Protocol`, because browsers cannot set
  headers: `cycling.v2, auth.<token>` for the agent and `cycling.v2, own.<key>`
  for the browser. The relay echoes only `cycling.v2`.
- Admission failures are delivered as accept-then-close with an application
  close code (`CloseCode`), because a browser cannot read the HTTP status of a
  refused upgrade.
- Messages. Browser: `DeviceAnnounced`, `PortChanged`, `OpProgress{id, done,
  total}`, `OpResult{id, outcome}`, `Bye`. Agent: `Op{id, op}`. Relay:
  `Welcome{role, snapshot}`, `PeerPresence`. Each client decodes the union of
  what it may receive, so a wrong-direction message fails at the boundary.
- Ops (`ops.ts`) are Device's verbs, each under a `Lease` the agent mints:
  `Enter`, `Read{region}`, `Write{region, md5}`, `Digest{region}`, `Reboot`,
  `Release`. There is no eFuse op, no erase, no register access and no line
  toggling, so none can be asked of a visitor's device. `Region` is checked
  against the 16 MiB flash at decode on every end. Outcomes: `Entered{chip}`,
  `Read{md5}`, `Written{md5}`, `Digest{md5}`, `Rebooting`, `Released`,
  `Failed{reason, detail}`; `detail` is esptool-js's words with MAC-shaped text
  redacted, at most 200 characters. `PortState` gains `Flashing`: the page's
  flasher holds the port and no console bytes flow.
- Liveness instead of timeouts: the page sends `OpProgress` at least once a
  second while an op runs; the agent fails an op `stalled` after 20 s with
  neither progress nor a result. No constant is tuned to a round trip.
- A session's end is its close code: `visitorDisconnected` 4000,
  `visitorGone` 4001, `expired` 4408, named after the `EndReason`.
  `closeMeaning(code)` is the one reading of every code for both clients:
  `Transient | Ended | Replaced | Refused | Remint`.
- `Device` is `serial{vendorId, productId}` or `usb{descriptor}`. No schema has
  a serial-number field, and strict decoding refuses one instead of dropping it.

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
  -> Remote.attach(code): RemotePort     epochs, port up/down, op ids and their answers
       -> pty door       Bun.Terminal, CYCLING_PORT = <dir>/<CODE>/port
       -> device socket  CYCLING_FLASHER = <dir>/<CODE>/device.sock -> leaseFlasher
```

- The pty is `Bun.Terminal` in-process. The bridge holds the slave open, so a
  tool closing the pty never hangs it up. The backlog toward the pty is a
  bounded dropping queue, so a pty with no reader never stalls the relay.
- While the port is down or `Flashing`, writes are dropped and counted; a pty
  cannot report EIO on demand, and the tools already tolerate gaps. The pty
  never sees SLIP.
- The device socket speaks JSON lines, one request then one reply:
  `enter`, `read{address, size, path}`, `write{address, path}`,
  `verify{address, path}`, `reboot{seconds, path}`. Paths are absolute files
  on this machine; the socket lives in the 0700 session directory. One
  connection is one lease, released (`Op Release`) when the connection closes,
  so a Python process that exits, crashes or is killed gives the device back.
  A second connection meanwhile is answered `busy`.
- `leaseFlasher` owns integrity and retries. A write announces the file's MD5
  before its bytes and accepts only the chip's matching digest; a read is
  accepted only when its frames are contiguous and the file's MD5 is the
  chip's. A relay drop or a broken transfer resends the op (every op but
  Reboot is idempotent under one lease) once the visitor is reachable again,
  waiting at most 30 s. One stderr line per op, never the MAC.
- Reboot subscribes to the console before it asks, waits up to 10 s for the
  port to be Open again, then copies `seconds` of console output into the
  file Python named, which is how `Device.reboot`'s boot capture works here.
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
- One port ManagedResource, one owner. Its requirement is
  `Console{identity, baud, attempt}` or `Flasher{identity, attempt, entry}`;
  switching is a re-key, and Foldkit closes the old resource's scope before it
  acquires the new one, so the console's reader is gone and the port closed
  before esptool-js's Transport opens it. Acquiring the flasher is
  `serial.granted` (the picked SerialPort, closed, waiting like reacquire)
  then `flasher.open`, then the device's own partition table and otadata.
- The hold is a port phase: `Entering{lease, entry}` and `Flashing{lease,
  layout}` sit beside `Open`, `Reacquiring` and `Failed`, so a hold on a
  failed port cannot be represented. `holder.ts` is the op table as pure
  functions, and it is the only place that answers an op, so every op gets
  exactly one `OpResult`. The table is its doc comment.
- One exit path. The flasher session's scope finalizer (in
  `@cycling/flasher`) is the only way from the flasher back to the console:
  unless a Reboot already reset the chip it waits for an abandoned esptool-js
  call to settle, sends python esptool's watchdog reset, and closes the port.
  Release, the visitor's Restart device, a failed op and the agent-away
  timeout are all "back to the console" in the Model; the re-key does the
  rest, so a stranger's device is not left in download mode.
- The op runner (`flasher/run.ts`) is a Subscription keyed on the head op. It
  puts `OpData` and progress on the relay itself and hands `update` redacted
  Messages. `Entered{chip}` is sent by a Command that reads `session.chip`
  from the port handle, so the MAC never passes through `update` or the
  Model. Write payloads wait in an inbox outside `update`, checked for holes
  (`transferGap`) and against the announced MD5 (`payloadMismatch`) before
  flash is touched.
- The agent leaving mid-hold changes nothing for 30 s: a running op
  finishes, and an `Enter` with the same lease re-answers without a reset.
  After 30 s with no agent the page goes back to the console.
- The visitor sees an update-mode panel in the device's own partition names,
  the last 20 ops, Restart device while update mode is idle, a Disconnect
  that asks first during a write (worded by what the write overlaps), and one
  consent card before the first write of the session. Allow lasts the
  session (`StoredSession.writesAllowed`).

## Tradeoffs accepted

- A custom framing (channel-byte binary plus JSON text) instead of base64
  envelopes.
- No replay across a relay drop: bytes in flight are lost, and presence and
  `Lost` tell both sides. A stale queue written into a device after a reset is
  worse than a gap.
- esptool-js, not python esptool, talks to the chip through the page. Only
  Device's five verbs exist there: `python -m esptool <anything>` (chip-id,
  read-mac, erase-region, eFuse reads) does not work through the page, by
  design. Adding a verb is a union member, a runner case and a Python method.
- The page holds a whole write payload (at most 16 MiB) before it writes, so
  the relay needs no flow control and a damaged payload never reaches flash.
- The agent token is in `Sec-WebSocket-Protocol`, visible to Cloudflare like
  any header.
- The MAC crosses the relay to the authenticated agent inside `Entered`.
- A 40-bit code; secrecy is the token and the owner key, and rate limits guard
  enumeration.

Rejected: a `socket://` leg (no modem lines, `--before usb-reset` silently does
nothing), python esptool over RFC 2217 through the relay (#345: the ROM's
timeouts cannot survive the round trip; that door is deleted), raw ROM
commands across the relay (a round trip per register), whole jobs on the page
(the policy in firmware.py would need a second copy), a stateful relay with an
in-memory peer map (breaks on hibernation), base64 envelopes, bytes through
`update`, two sockets per role (reordering).

## Implementation reconciliation

Where the code differs from the sketch, and why.

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
  `Sec-WebSocket-Protocol`; the relay sets `cycling.v2` on the 101 itself
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
  device socket is an `@effect/platform-bun` socket server, its `-- <cmd>`
  child is an Effect `ChildProcess`. The agent refuses
  a relay that does not select `cycling.v2`, a token shorter than 32
  characters, and `ws://` to anything but loopback. The child never sees the
  token. Extra exit codes: 5 port did not open in 30 s, 1 local resource
  failure, 127 the command did not start; a session that ends while waiting
  exits with the ending's code.
- **Paths.** The launcher (`device/browser_bridge.py`) passes
  `CYCLING_REMOTE_DIR` = the main checkout's `.local/bridge` (token at
  `.local/bridge/token`); a session lives
  in `<dir>/<CODE>/` (`port` symlink, `device.sock`). `flasher_for` picks
  `PageFlasher` only when the selected port is the bridge's `CYCLING_PORT`
  and `CYCLING_FLASHER` is set, so a second, local device in a bridged
  command stays on `EsptoolFlasher`. Checks run inside
  `cycling check` and a `test-web` task in `cycling test`.
- **Site.** The serial port is read whenever it is open, even while the relay
  is down (bytes are dropped and counted), because Chrome's receive buffer
  overruns when nobody reads. `sessionStorage` also keeps baud and epoch, so a
  reload does not restart epochs. Bytes toward the port wait in a queue of 32
  frames written by their own fiber, so control and a close are handled while
  a write is stuck; a close drops what is queued and counts it. On reacquire the page reopens the port object the visitor
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
real edge; the re-enumeration gap against the 25 s boot budget; the browser
can send `Lost` ahead of the last coalesced bytes; the agent's outbound queue
toward the relay is unbounded (R9). The page flasher's own risks are listed at
the end of its reconciliation below.

## Page flasher reconciliation (#345)

Where the code differs from the bakeoff's synthesis (candidate 2 with its
grafts), and why.

- **Watchdog decision by VID/PID.** Python esptool 5.4.0 disables the S3
  watchdogs when `uses_usb_jtag_serial()`, which is the host's view of the
  port (303a:1001), not the `UARTDEV_BUF_NO` register the research notes
  quoted. The flasher asks `port.getInfo()` the same question. A devtools
  test (`tests/test_flasher_watchdog_tables.py`) runs esptool's own
  `disable_watchdogs` and `watchdog_reset` against a recording chip and
  compares the writes with `web/flasher/src/watchdog.ts`; it fails on any
  changed literal.
- **Write check.** `write` does not hand esptool-js a `calculateMD5Hash`;
  esptool-js reports its own mismatch only as a generic message. The flasher
  compares the chip's MD5 of the written range itself (`digestMismatch`).
- **Errors by phase.** esptool-js has no error class worth telling apart, so
  a failure during `open` is `entryFailed`, later `chipError`, and anything
  once the port's `disconnect` event fired is `portLost`. esptool-js 0.7.1's
  device-lost callback never fires (its read loop tests `instanceof Error`
  before `DOMException`), so port loss comes from the port's own event, and
  every esptool-js call is raced against it.
- **Read pieces are 256 KiB**, not 1 MiB: about 2 s of the chip's ~140 KB/s,
  so an abandoned piece settles inside the finalizer's 4 s wait and a Release
  mid-read still gets its recovery reset. In the simulator a 16 MiB read
  takes about 7 s, bounded by esptool-js re-copying its buffer per packet.
- **The hold is a port phase, not `Port.holder`.** `Entering` and `Flashing`
  phases replace C2's separate holder, which ruled out a hold on a failed
  port.
- **Only `update` answers ops.** `Entered` goes out through a Command that
  reads the chip from the handle, not from the runner, so a Release racing a
  running op cannot produce a second answer.
- **Which failures end a hold.** `chipError`, `portLost` and `entryFailed`
  end it; `payloadMismatch`, `transferGap`, `digestMismatch` and `notAllowed`
  keep it, because the agent resends those.
- **A port lost during a Reboot is the reboot.** The watchdog reset can drop
  the port before the op's own result arrives; the page answers `Rebooting`
  then, not `portLost` (found by the end-to-end run).
- **Queued Enters wait for the console.** An Enter behind a Reboot starts once
  the console is open again, not while the device is still re-enumerating.
  A second heartbeat covers a head op that waits (entering, consent, console),
  so a slow visitor does not trip the agent's 20 s stall.
- **The agent sends one op at a time** per lease (the device socket is
  sequential), so its stall clock never runs for an op queued behind another.
- **`detail` is redacted text, not "no text".** Failed outcomes keep a
  200-character, MAC-redacted detail for the agent's private log and the
  Python error; it never reaches the page's Model or view.
- **The MAC is 12 hex characters in `Entered`**, a branded `Mac` string rather
  than a byte array, so it stays one typed field in a JSON message.
- **Acquire is uninterruptible.** Effect's `acquireRelease` runs the flasher
  acquire uninterruptibly, so a Release or Disconnect during `Entering` waits
  for that entry attempt to finish (at worst the 60 s reacquire wait).

Verified device-free: `bun run check` (protocol, relay, agent, site, flasher
against the simulator, including a 16 MiB read, a write and its digest, and a
watchdog model that resets a chip whose watchdogs were not disabled),
devtools' pytest, and `web/agent/test/e2e-page.sh`: the built site in headless
Chromium with the simulator as its Web Serial port, the offline relay, the
real `cycling remote` and devtools' `Device` through `PageFlasher` (enter,
read 0x8000, write and verify a scratch region, read it back, reboot with the
boot capture), plus killing the device task mid-read (Release, back to the
console, chip in its app).

Open risks for hardware, cheapest experiment first:

- **Re-enumeration on entry.** Whether `usb_reset` makes the C606 re-enumerate
  is unmeasured; the page retries once with `noReset` if it does. Whether
  Chrome hands back the same `SerialPort` object after a watchdog reset is
  also unmeasured; the page reopens the picked object when it reappears, else
  the first unopened match.
- **Background tabs.** esptool-js polls with `sleep(1)`; Chrome throttles
  timers in hidden tabs. The page asks the visitor to keep it in front; a
  hidden-tab 16 MiB read is the experiment.
- **Relay throughput for 16 MiB.** About 1,100 OpData frames each way per full
  read; the chip's ~140 KB/s should bind, not the relay or the visitor's
  uplink, but this is inferred, not measured.
- **Boot capture.** It starts after re-enumeration, Chrome's reacquire and the
  relay, so the first ROM lines are likelier to be missed than locally.
- **Recovery after an interrupted op** waits at most 4 s for the abandoned
  esptool-js call; a slower chip leaves the device in the stub until a replug.
- **A stranger's device** still cannot run `cycling backup` or `flash`:
  `.local/device` holds one device's private state (follow-up).
