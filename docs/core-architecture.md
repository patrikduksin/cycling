# Core contracts and migration inventory

Initial decision for #55, audited against eb0c7d2 on 2026-09-12. See
[implemented architecture](architecture.md) for final ownership and bounds. These were implementation
contracts for #54, not a claim that the target architecture or build matrix works
already. Keep modules in `packages/os`; change this document when working code
shows a simpler boundary.

## Dependencies and ownership

`main` composes `device`, `core`, optional `sdk`, and `terminal`. The existing
Embassy executor, time, synchronization and ESP integration remain the runtime.
No service registry, universal event bus or replacement executor is needed.

- `device::c606` alone initializes pins, clocks, DMA, interrupts, PSRAM, radio,
  flash and reset hardware. It transfers each peripheral once to its owner.
  Board drivers expose capabilities and transport facts, not app state.
- `core` owns position acquisition/validity, generic BLE transport, connectivity,
  bounded persistence, display submission, physical input, battery reports,
  settings, time, power and diagnostics. It may use device drivers and Embassy.
- `sdk`, behind a `cycling` feature, owns HRS/CSC interpretation, ride state,
  sampling, history, ride encoding, verified reclaim and export semantics.
  It consumes core capabilities. Core storage does not import ride types.
- `terminal` parses bounded commands, dispatches to core or feature-gated SDK
  handlers, and serializes replies. It runs with `debug-harness` disabled.
  The harness is a separate consumer for injection/capture and test controls.

Device and core must not import SDK, UI, demo or harness policy. A driver must
not refer to `coin::PIXELS`, `ui::App`, `ride_log`, or `debug` to obtain a general
capability. Prefer private modules and narrow exports. The base build must
exclude SDK modules with `#[cfg(feature = "cycling")]`; a textual directory move
alone does not enforce that boundary. Future apps and a graphical shell are out
of this queue's scope.

The current violations are concrete. `main.rs` configures hardware and also
polls input/GNSS, renders, samples rides, persists settings and dispatches USB.
`metrics::Snapshot` combines crash, position, sensors, rides, display and harness
state. `bluetooth.rs` owns both the controller/GATT lifecycle and HRS/CSC choice
and decoding. `persistent.rs` owns flash but imports preferences and ride slot
formats. `ride_recorder.rs` combines SDK scheduling with media operations.
`debug_usb.rs` depends on the entire UI snapshot, idle policy, persistence and
rides. `display.rs` obtains canvas geometry from the coin demo and coordinate
mapping from screenshot code. Split these dependencies before deleting callers.

## First interfaces

Use service-specific structs and enums. A position snapshot carries measurement
and receipt times, validity and transport-loss counters; a GNSS owner advances
it independently of rendering, terminal traffic and ride sampling. Core BLE
reports connection state and bounded GATT bytes; SDK decoders own heart rate,
cadence, wheel circumference and sensor freshness. Core display accepts an
explicit RGB565 extent and buffer, core input reports physical transitions,
and core battery reports available raw/decoded companion measurements without
inventing a calibrated capacity estimate.

For snapshots distinguish `Unavailable` before a usable observation, `Stale`
with retained value and age after its freshness deadline, `Unsupported` when
the device/build lacks a capability, and `Failed` with a bounded error code when
an attempted operation fails. These need not form one shared enum for every
service. A receiver reporting no fix is available transport with an invalid
position, not a valid zero coordinate. A disconnected sensor is not zero BPM.
Link-down must remain distinct from a failed internet probe. Preserve existing
GNSS epoch validity, sensor freshness and time anchoring rules during extraction.

Use Embassy primitives directly with these initial bounds. They are budgets to
implement and check, not measured allocations or a public SDK promise.

| Traffic | Delivery and storage | Failure, cancellation and lag |
| --- | --- | --- |
| Position, battery, link, time, diagnostics | One latest snapshot per service, copied under a short mutex or published through `Watch`; include sequence and monotonic observation time | New values replace old values. Readers detect skipped sequences and stale age; this is not a sample archive. |
| Commands requiring completion, including connect, settings save and storage mutation | One owner, bounded `Channel` of four small requests and one outstanding request per caller; request ID and explicit accepted/completed/error reply | Full means `Busy`, never overwrite. Retain completion until caller consumes it. Caller timeout does not imply a completed write was undone. Reject a second command while its reply slot is occupied. |
| Input edges | Queue of 16 events, timestamp and sequence | Overflow increments loss count and forces consumer release/cancel before resuming; never silently leave a held press. Harness injection stays above physical input. |
| BLE notifications | Retain Trouble's configured two-entry notification queue and one-peer limit initially | Count detectable overflow and disconnects; SDK invalidates continuity on loss and uses age deadlines. Do not promise every notification or concurrent separate sensors. |
| Display | One in-flight submission, explicit byte/extent bound | Return buffer ownership only after transfer completes or driver quiesces it. Reject concurrent submission as busy; no retained frame history in the base. |
| Logs | Bounded records, 384 bytes each; eight queued records initially | Drop with a counter/sequence gap when full. No heap-growing queue and no blocking peripheral task behind USB output. |

Choose `Signal` only when there is one consumer and replacement is intended;
use `Watch` or copied snapshots for multiple readers. Channels carry requests
or edges, not every changing metric. Do not hold a mutex across `.await` or
flash/DMA work. Keep reply payloads fixed-size; pass storage buffers by exclusive
ownership instead of placing whole sectors in every queued request.

Every externally initiated wait has a deadline. Retain the existing Wi-Fi
connect/DHCP 20 s, disconnect 5 s, probe 15 s and SNTP 10 s limits and capped
backoff. Retain BLE connect 12 s and discovery/subscription 10 s limits until
changed-path evidence warrants changes. Cancel read-only requests by dropping
their wait; owners still release resources. A queued mutation may be cancelled
before acceptance. After acceptance, finish the current bounded media operation,
report its result, and stop at a safe boundary. Never blindly retry an ambiguous
write/erase. A terminal timeout returns the request ID and unknown completion
state so a client can inspect status. Driver timeouts must quiesce DMA before
reusing buffers. Reconnect generation changes discard results from older links.

Keep the current 8 KiB GNSS DMA ring, 256-byte GNSS read chunk and 2 KiB companion
interrupt ring initially. Reset parsers before consuming post-loss bytes. Count
transport faults separately from parser rejects. Account for every new static
queue with `size_of`, task storage and internal/external placement in its PR.
Preserve the existing 160 KiB internal heap configuration until measured changes
justify it; allocation failure is explicit. Existing harness capture storage is
33.125 KiB and redraw comparison uses 16,960 external bytes. These are migration
baselines, not a base-firmware requirement. Measure heap minima as sampled
minima, never as peak memory. Compare performance using harness-disabled builds.

For #66 use the `log` facade and bounded JSON Lines records containing a boot/session marker,
monotonic milliseconds, sequence, source, level and event plus bounded fields.
Host receipt time is separate. Include firmware/build mode and recording state.
One USB writer arbitrates logs and command replies, with replies prioritized.
The recorder handles partial lines, reconnects and gaps and writes raw output
only under ignored `.local/`. Preserve panic/reset reporting even if normal
logging cannot run; exclude credentials and peer/device identifiers from public
findings. Keep serialization and the USB writer separate from service logic.

## Persistence is a compatibility boundary

Preserve stock ota_0, bootloader, partition table, eFuses and existing data.
Keep the owned ride reservation `0xd98000..0xe98000` and settings reservation
`0xe98000..0xe9a000`, with existing flasher image limits. Core exposes checked
relative reads/program/erase only within named owned reservations; offsets,
alignment and overflow checks precede access. It provides exclusive flash
ownership and truthful completion, while SDK retains ride scanning, append,
commit and reclaim policy. No vendor filesystem writes or automatic format.

Retain settings journal magic `C606`, format version 1, commit ordering/checksum
and both sectors. Preserve preference decoding of previous supported versions
and current `cycling` prefix/version 4 bytes despite the product-oriented name.
Move preference fields into core without renaming on-flash bytes. Retain ride
slot encoding, commit/checksum rules, recovery scan and export identity checks.
An unsupported/corrupt format is reported and preserved, never rewritten as an
empty journal. Existing fake-media torn-write, interrupted-clear and refusal
tests follow their owners. Read pre-refactor bytes with the new code, compare
exports, reboot and reflash safely before claiming compatibility on hardware.

## Exact migration inventory

Paths below are relative to `packages/os/src` unless a different prefix is shown.
Grouped entries have the same disposition. Move means retain behavior and tests;
replace means keep the old path until the named replacement is verified.

| Existing files | Disposition and removal gate |
| --- | --- |
| `main.rs`, `lib.rs` | Replace composition/exports after device/core/SDK owners exist; base boot and import/feature checks prove the split. |
| `companion.rs`, `companion_uart.rs`, `uart_ring.rs`, `gps_uart.rs`, `touch.rs`, `psram.rs`, `crash_rtc.rs`, `sdmmc.rs`, `sdmmc_probe.rs` | Move to device support; retain protocol/ring/probe tests and pin/timing behavior. GNSS scheduling moves to core. MMC remains opt-in read-only. |
| `display.rs` | Move driver to device; replace demo geometry/screenshot dependency with explicit display extent; test mapping and observe panel output before deleting old mapping. |
| `gps.rs`, `input.rs`, `network.rs`, `network_time.rs`, `crash.rs`, `storage.rs` | Move portable general logic/tests to core; preserve existing validity, loss, timeout, journal and reset behavior. |
| `wifi.rs` | Move connectivity/time tasks to core and radio creation to device; remove harness fault policy from service owner after harness calls a bounded test adapter. |
| `preferences.rs`, `idle.rs` | Move settings format/debounce and general power policy to core; replace UI session/wake gesture coupling with explicit client policy; preserve format and idle tests. |
| `persistent.rs` | Replace with device flash backend and core bounded storage; SDK adapters preserve ride operations. Require fake-media bounds/recovery tests and unchanged device data. |
| `bluetooth.rs`, `ble_sensor.rs` | Split device radio ownership, core BLE/GATT, SDK sensor selection/interpretation; preserve MTU23 and decoder tests plus reconnect/freshness evidence. |
| `ride.rs`, `ride_log.rs`, `ride_reclaim.rs`, `ride_recorder.rs` | Move non-demo workflows/formats/tests to SDK. Replace direct flash use with core reservations; delete deterministic demo source only after real/source-controlled sampling tests exist. |
| `metrics.rs` | Replace combined snapshot with core diagnostics and SDK status; delete UI number formatting after terminal diagnostics checks. |
| `debug.rs`, `debug_usb.rs` | Replace mixed protocol/UI authority with supported terminal plus optional harness; preserve framing, request correlation, lease cleanup, export and loss regression coverage before removing old commands. |
| `screenshot.rs`, `redraw.rs` | Final #64 decision: remove capture/recording and shell redraw code because the base has no UI capture consumer. Preserve coordinate mapping in tested generic display capabilities and validate explicit display submission. |
| `ui.rs`, `controls.rs`, `coin.rs`, `packages/os/examples/preview.rs` | Delete in #64 after terminal covers settings/status/ride workflows and input/display capabilities have focused tests. Extract required geometry first. Retire shell-only unit tests with their code. |
| `packages/os/assets/{README.md,rust-logo.mask,rust-logo.svg}`, `docs/demo.gif`, `scripts/logo.py` | Delete in #64 after removing coin/preview references; no replacement artwork needed. |
| `scripts/build.sh`, `scripts/device.py`, `scripts/test_device.py` | Retain safe image/backup/flash/stock logic; extend feature matrix and keep protected-region tests. |
| `scripts/{wifi.py,ble_config.py,test_wifi.py,test_ble_config.py,bluetooth.py,ble_sensor_sim.py}` | Retain configuration privacy and radio fixtures; update transport callers where needed and rerun affected tests. |
| `scripts/{debug.py,test_debug.py,gps_stress.py,test_gps_stress.py,companion_stress.py,persistence_test.py}` | Replace UI/debug protocol use with terminal/harness client; preserve framing/handoff, stress counters and persistence restart coverage before deleting paths. |
| `scripts/{export_rides.py,test_export_rides.py,clear_rides.py,test_clear_rides.py}` | Retain SDK export/reclaim tooling and tests; migrate transport after byte-identical export and clear-refusal tests pass. |
| `scripts/{screenshot.py,test_screenshot.py}` | Final #64 decision: remove with the unused UI capture protocol. Terminal/display-transfer checks and mapping tests replace the relevant base coverage. |
| `scripts/{visual.py,test_visual.py,regression.py,test_regression.py}` | Replace useful soak/reconnect/loss checks with service/terminal tests, then delete shell visual comparison/navigation code and its tests. |
| `scripts/scenarios/{navigation,diagnostics,device-screens,controls,idle-dimming,input}.json` | Delete shell scenarios after status/settings, idle and physical-input contracts have replacement checks; injected gestures do not prove physical switches. |
| `packages/os/vendor/trouble-host/**` | Retain complete licensed pinned source, patch note, lockfile, scripts and tests. No vendor cleanup in this refactor. |
| `packages/stock/magene-c606/**` | Retain all written research, partition CSV and `unpack.py`; verified hardware and candidate drivers remain distinguished. |
| `Cargo.toml`, `Cargo.lock`, `packages/os/Cargo.toml`, `.cargo/config.toml`, `.github/workflows/check.yml`, `.gitignore`, `mise.toml` | Retain; update features/check matrix, commit intentional lock changes, keep private evidence ignored. |
| `README.md`, `docs/{README,development,device,device-debugging,ride-recording,overnight-plan}.md`, `AGENTS.md`, `LICENSE` | Retain; replace obsolete shell instructions/evidence links with actual terminal workflows in #64/#65. Preserve useful history and license. |

All inline Rust tests move with retained logic. For split modules, keep protocol,
media, validity and loss tests with the lower owner; keep ride/sensor tests with
SDK; replace shell-coupled tests before deleting them. No blanket test deletion.

Mise task disposition is explicit. Retain `setup`, `build`, `test`, `fmt`, `check`,
`backup`, `flash`, `stock`, `monitor`, `wifi-setup`, `ride-export`,
`ride-clear`, `gps-stress`, `companion-stress`, `bluetooth-echo`, `ble-simulator`.
Replace `debug`, `e2e`, `persistence-test`, `regression`, `crash-test` with supported
terminal/service checks before retiring old entry points. Delete `preview` when
its example and shell go. Add terminal/log recording tasks with their owning
issues; do not leave descriptions promising removed behavior.

## Sequence and completion checks

Land #55, then #56 device ownership. Deliver #66 logging alongside #57 position,
#58 connectivity, #59 persistence, #60 display/input/battery and #61 general
services. #62 gates SDK, #63 supplies the terminal, #64 deletes replaced shell
code/tooling, and #65 validates the result. Temporary legacy callers are allowed
during extraction, but device/core must not gain new reverse dependencies.

Required target matrix, not yet validated by this document:

| Build | Required observation |
| --- | --- |
| Host base, no default features | Core/device-independent tests compile without SDK/UI/harness imports. |
| Host with `cycling` | Existing sensor/ride/recovery/export logic remains tested. |
| C606 base, harness off/on | Both boot with terminal/logging, position/input/status/storage inspection and no UI/SDK dependency. Capture/injection only exist in the enabled build. |
| C606 with `cycling`, harness off/on | Both boot; SDK workflows use the same core services and existing data is readable. |

For firmware changes run `mise run test`, `mise run check`, `mise run build` and
`CYCLING_HARNESS=0 mise run build`; extend build selection for the four C606 rows
when `cycling` lands. Observe changed peripheral access and display timing on
the authorized device. Record progress during terminal/USB load, bounded error
recovery, reply correlation and unchanged settings/ride exports. Report actual
counts, intervals, heap samples, build mode and recording state. Hardware limits
remain open if unobserved. Use safe repository device tasks, serialize access,
and restore the harness-enabled development build per the active session plan.
