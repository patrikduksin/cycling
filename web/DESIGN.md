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
  against the 16 MiB flash at decode on every end, and on the page against the
  flash size the chip reports (`pastFlash`). Any family esptool-js detects is
  entered; `unsupportedChip` is left for a flash that does not report its
  size. Outcomes: `Entered{chip}`, `Read{md5}`, `Written{md5}`,
  `Digest{md5}`, `Rebooting`, `Released`, `Failed{reason, detail}`; `detail`
  is esptool-js's words with MAC-shaped text redacted, at most 200
  characters. `chip` is the MAC, the family (`ChipFamily`, esptool-js's
  `CHIP_NAME`s), model, revision, features, crystal MHz, the flash's JEDEC
  manufacturer, device and size, the port's USB ids (or null), and
  `security`: `Known{secureBoot, flashEncryption}` or `Unknown` where the ROM
  has no GET_SECURITY_INFO (ESP8266, ESP32). `PortState` gains `Flashing`: the page's
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
  A second connection while one is open is answered `busy`; one that arrives
  while a closed connection's lease is still being released waits for it.
- `leaseFlasher` owns integrity and retries. A write announces the file's MD5
  before its bytes and accepts only the chip's matching digest; a read is
  accepted only when its frames are contiguous and the file's MD5 is the
  chip's. Every op but Reboot is idempotent under one lease and is tried up
  to three more times, each once the visitor is reachable again (at most
  30 s). An op whose answer never came (a relay drop, 20 s of silence) is
  resent under its own request id, which the page joins while the op runs or
  answers again once it is done, so a long write is never run twice or
  queued behind itself. One whose answer was unusable (broken read data,
  `transferGap`, `payloadMismatch`, `digestMismatch`) is tried again under a
  new id. One stderr line per op, never the MAC.
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
  entry, layout, answered}` sit beside `Open`, `Reacquiring` and `Failed`, so
  a hold on a failed port cannot be represented. `holder.ts` is the op table
  as pure functions, and it is the only place that answers an op, so every op
  gets exactly one `OpResult` (a resend of it joins it or gets that answer
  again from `answered`). The table is its doc comment. `headState` is the one
  reading of the hold (Entering, Running, AwaitingConsent, Idle) for the
  runner, the heartbeats, the hold's expiry, the update panel and the
  visitor's buttons.
- One exit path. The flasher session's scope finalizer (in
  `@cycling/flasher`) is the only way from the flasher back to the console:
  unless a Reboot already reset the chip, it waits for the abandoned
  esptool-js call to settle (a write stops at its next block, a read piece
  takes about 2 s), then sends python esptool's watchdog reset, and closes
  the port. The session runs one esptool-js call at a time, so the reset
  never interleaves with the abandoned call; if that call has not settled in
  30 s the reset is skipped, not raced. Release, the visitor's Restart
  device, a failed op and an expired hold are all "back to the console" in
  the Model; the re-key does the rest, so a stranger's device is not left in
  download mode.
- The op runner (`flasher/run.ts`) is a Subscription keyed on the head op. It
  puts `OpData` and progress on the relay itself and hands `update` redacted
  Messages. `Entered{chip}` is sent by a Command that reads `session.chip`
  from the port handle, so the MAC never passes through `update` or the
  Model. Write payloads wait in an inbox outside `update`, checked for holes
  (`transferGap`) and against the announced MD5 (`payloadMismatch`) before
  flash is touched.
- A hold never ends under a running op. `holdExpiry` gives the device back
  30 s after the agent left (or the page lost the relay) once nothing runs,
  and after 2 min of an idle hold with the agent present, which is how a
  Release lost on the way still returns the device to its app. Meanwhile an
  `Enter` with the same lease re-answers without a reset.
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
  in `<dir>/<CODE>/` (`port` symlink, `device.sock`). `firmware.flasher_for` picks
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

- **Watchdog decision by VID/PID.** Python esptool 5.4.0 disables the
  watchdogs when `uses_usb_jtag_serial()`, which is the host's view of the
  port (303a:1001), not the `UARTDEV_BUF_NO` register the research notes
  quoted. The flasher asks `port.getInfo()` the same question, and
  `uses_usb_otg()`'s (Espressif's VID with the family's image chip id);
  any other port is a USB-UART bridge.
- **Per-family tables, checked against python.** `web/flasher/src/families.ts`
  holds, for each of the 15 families esptool-js 0.7.1 detects, what python
  esptool does and esptool-js does not: the writes `_post_connect` makes on
  USB-Serial-JTAG, the `watchdog_reset` `--after watchdog-reset` runs on the
  chip's own USB (python falls back to `hard_reset` where a family has none:
  ESP8266, ESP32, C6, H2, H21, H4, E22), and what `hard_reset` writes before
  its RTS pulse on a bridge (only the S3 clears its force-download bit). A
  devtools test (`tests/test_flasher_watchdog_tables.py`) runs python's own
  methods for every family against a chip whose registers read 0 and a
  recording port, and fails on any changed literal, a missing family, or a
  change to esp_pylib's flow-control bridge list.
- **UART bridges.** On a CP210x, CH340 or FTDI board entry is esptool-js's
  ClassicReset (its `default_reset` picks it for anything but 303a:1001), and
  a reboot or the recovery reset is python's `hard_reset`: RTS held 100 ms,
  DTR re-sent after each RTS change as python does for Windows. A CP2102C
  (10c4:ea64), whose CTS follows RTS, gets python's flow-control pulse as it
  runs where HUPCL cannot be cleared, which is the browser's case. The port
  never leaves the bus, and the console reopens the same SerialPort.
- **Two deviations from python.** On an E22 over USB-OTG python's
  `watchdog_reset` and `hard_reset` call each other while GPIO0 reads low;
  the page uses the USB-Serial-JTAG sequence there. The page also runs
  esptool-js's own `postConnect` (the OTG RAM block, the P4's flash power),
  so on a P4 over USB-Serial-JTAG the watchdog writes happen twice; they are
  idempotent.
- **Write check.** `write` does not hand esptool-js a `calculateMD5Hash`;
  esptool-js reports its own mismatch only as a generic message. The flasher
  compares the chip's MD5 of the written range itself (`digestMismatch`), so
  the page answers `Written` with the announced MD5 its inbox already checked
  and runs no second digest.
- **Reads check the stub's own digest.** esptool-js 0.7.1's `readFlash`
  drops the MD5 the stub sends after the data and copies its whole buffer per
  packet. The session reads with python esptool's framing (4 KiB packets, 64
  in flight), checks each piece against that digest, and ends a read with the
  region's MD5 of verified pieces. No SPI_FLASH_MD5 runs per piece or over
  the whole region, which saves the chip two hashing passes per read.
- **Errors by phase.** esptool-js has no error class worth telling apart, so
  a failure during `open` is `entryFailed`, later `chipError`, and anything
  once the port's `disconnect` event fired is `portLost`. esptool-js 0.7.1's
  device-lost callback never fires (its read loop tests `instanceof Error`
  before `DOMException`), so port loss comes from the port's own event, and
  every esptool-js call is raced against it.
- **Read pieces are 256 KiB**, not 1 MiB: about 2 s of the chip's ~140 KB/s,
  so an abandoned piece settles quickly and a Release mid-read still gets its
  recovery reset.
- **Read throughput, measured device-free.** The simulator hands the page
  Chrome's 255-byte reads and paces READ_FLASH (`readBytesPerSecond`). At the
  C606's 140 KB/s a read runs at 98-99% of it, both through
  `web/flasher/test/read-bench.ts` and end to end in headless Chromium with
  the relay, agent and devtools (`E2E_BENCH_MIB=4 sh
  web/agent/test/e2e-page.sh`: 4 MiB in 30.6 s, 137 KB/s). With no chip
  limit the host side alone moves 2.4 MB/s in the flasher and 750 KB/s end
  to end (16 MiB in 22.4 s), so the chip binds, not esptool-js's buffer
  copies, the relay or the agent. A real 16 MiB read through the page that
  ran for 10 min is not explained by anything the simulator models; the
  untested suspects are a hidden or occluded tab (esptool-js waits with
  `sleep(1)` timers, which Chrome throttles to 1 s and then to 1 min) and
  the chip's own hashing, which the two digest passes above added.
- **Chip description and flash size.** Entry describes the chip from
  esptool-js (`getChipDescription`, split into model and revision,
  `getChipFeatures`, `getCrystalFreq`, `readMac`), GET_SECURITY_INFO where
  the ROM has it, and one RDID: manufacturer, device and the size its
  capacity byte names, as python's flash-id splits them. Regions past that
  size are refused `pastFlash`. Python's `Chip` carries the same typed
  fields and `accepts_unsigned` (None when security is unknown); `Device`
  refuses a chip whose ROM cannot say, before any read.
- **Bootloader offset.** The session reports its family's
  `BOOTLOADER_FLASH_OFFSET` (0, 0x1000 on the ESP32 and S2, 0x2000 on the C5,
  H4, P4 and S31); the page's layout carries it, so "the bootloader" and the
  Disconnect warning start there. The layout is always present while
  flashing; a device without a readable table has no partitions in it.
- **`cycling flasher`.** `info|read|verify|reboot`, built on `flasher_for`,
  so the same verbs run on a local tty (EsptoolFlasher with `--chip auto`:
  `flash-id`, then `get-security-info` where the family has it) and through
  the page. `info` prints the description as JSON with a 12-digit sha256
  prefix of the MAC, which tells devices apart but is not anonymous (a known
  vendor prefix leaves 2^24 MACs to try). `read` writes only under `.local`.
  There is no write verb. Every verb ends with a reboot, so the device is back
  in its app. EsptoolFlasher's reboot now runs `read-mac`, which every family
  answers, under `--after watchdog-reset`.
- **A resend joins the op.** The agent resends an unanswered op under its
  own id; the page joins it while it is queued or running, and answers the
  last finished one again from `Flashing.answered`. Every queued op that is
  not running gets the once-a-second heartbeat, so nothing stalls behind a
  long op.
- **One esptool-js call at a time.** esptool-js flushes input per command,
  so the session chains its calls: each starts once the one before it
  settled, even when the fiber that started that one was interrupted.
- **The hold's expiry is a subscription.** `holdExpiry` replaces the
  departures counter, a WaitForAgent command and its message: 30 s after the
  agent left once nothing runs, 2 min of an idle hold with the agent here.
- **One flasher adapter.** `site/src/adapters/flasher.ts` turns a session
  into wire types once: the chip as the protocol's `Chip`, digests as `Md5`,
  details cut to 200 characters.
- **The hold is a port phase, not `Port.holder`.** `Entering` and `Flashing`
  phases replace C2's separate holder, which ruled out a hold on a failed
  port.
- **Only `update` answers ops.** `Entered` goes out through a Command that
  reads the chip from the handle, not from the runner, so a Release racing a
  running op cannot produce a second answer.
- **Which failures end a hold.** `chipError`, `portLost` and `entryFailed`
  end it; `payloadMismatch`, `transferGap`, `digestMismatch`, `pastFlash`
  and `notAllowed` keep it. The agent tries the first three again; it does
  not ask the visitor again after Not now.
- **A port lost during a Reboot is the reboot.** The watchdog reset can drop
  the port before the op's own result arrives; the page answers `Rebooting`
  then, not `portLost` (found by the end-to-end run).
- **Queued Enters wait for the console.** An Enter behind a Reboot starts once
  the console is open again, not while the device is still re-enumerating.
  A second heartbeat covers a head op that waits (entering, consent, console),
  so a slow visitor does not trip the agent's 20 s stall.
- **The agent sends one op at a time** per lease (the device socket is
  sequential); only its own resend can queue behind a running op.
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
real `cycling remote`, `cycling flasher info`, `read` and `verify`, then
devtools' `Device` through `PageFlasher` (enter, read 0x8000, write and
verify a scratch region, read it back, reboot with the boot capture), plus
killing the device task mid-read (Release, back to the console, chip in its
app), plus a classic ESP32 behind a CP2102N (info with security unknown,
Device's refusal, the console back after the RTS reboot). The simulator is an
ESP32-S3 or ESP32-C3 on USB-Serial-JTAG or an ESP32 on a UART bridge, each
with its own register map written from the TRMs; the flasher's tests enter,
describe, read, digest, outlast the watchdog and reboot each, and cover a
write abandoned mid-way (it stops at a block, then the reset, never
interleaved) and a 4 MiB chip refusing regions past its flash; the site's, a resend joining a running write and an idle or
unattended hold expiring only once nothing runs; the agent's, a lost answer
asked for again under its id (the page writes once) and a `transferGap`
write sent again.

Open risks for hardware, cheapest experiment first:

- **Re-enumeration on entry.** Whether `usb_reset` makes the C606 re-enumerate
  is unmeasured; the page retries once with `noReset` if it does. Whether
  Chrome hands back the same `SerialPort` object after a watchdog reset is
  also unmeasured; the page reopens the picked object when it reappears, else
  the first unopened match.
- **Background tabs, and the slow real read.** esptool-js polls with
  `sleep(1)`; Chrome throttles timers in hidden tabs. The page asks the
  visitor to keep it in front. The experiment: one 16 MiB read on the C606
  with the tab in front, one hidden, timed against the simulator's 137 KB/s.
- **Relay throughput over the internet.** Device-free, the local relay
  carries 750 KB/s end to end; the real edge and a visitor's uplink are
  unmeasured.
- **Boot capture.** It starts after re-enumeration, Chrome's reacquire and the
  relay, so the first ROM lines are likelier to be missed than locally.
- **Recovery after an interrupted op** waits up to 30 s for the abandoned
  esptool-js call; a call that never settles skips the reset rather than race
  it, and leaves the device in the stub until a replug.
- **A stranger's device** still cannot run `cycling backup` or `flash`:
  `.local/device` holds one device's private state (follow-up). `cycling
  flasher` works on any family.
- **Families beyond the simulator.** Only the S3, C3 and ESP32 run against
  the simulator; the other twelve families' tables are checked against
  python esptool but have not met a chip, through the page or here.
