# Implemented architecture

Firmware stays in `packages/os`. This document describes the current C606
implementation; `core-architecture.md` retains the original migration inventory.
A second board and a board-independent firmware binary have not been validated.

## Ownership and dependencies

`main.rs` composes the board, core services, terminal and optional cycling SDK.
`device/c606.rs` initializes concrete clocks, pins, PSRAM, DMA and peripherals,
then transfers each resource to one owner. Device modules implement UART, touch,
display, flash, retained crash state and the opt-in read-only MMC probe.

Core consists of the portable library modules plus `services/positioning.rs`,
`services/io.rs`, `wifi.rs`, `bluetooth.rs`, `persistent.rs` and `core_system.rs`.
It owns acquisition, settings, power policy, generic BLE bytes, networking/time,
display operations, physical observations and bounded storage. Device and core
cannot import SDK, shell or harness policy.

`src/sdk/` owns HRS/CSC decoding and freshness, ride lifecycle and sampling,
record encoding, recovery, summaries and reclaim policy. Its algorithms take
caller-provided media, snapshots and monotonic time. They have no HAL or UI
imports. `sdk_runtime.rs` is a composition adapter: it reads core snapshots,
drains BLE packets and calls the portable recorder before terminal dispatch.
The SDK retains demo provenance when decoding old records; deterministic demo
generation is a test fixture, with no terminal demo-start flow.

`terminal.rs` owns USB and dispatches core commands. Cycling builds also pass
`RIDE` and `EXPORT` commands to the SDK adapter. It is a consumer of both layers,
so it is intentionally outside the lower-layer import checks.

## Scheduling, buffers and failure

The existing Embassy executor, timers and synchronization remain the runtime.
Position and physical-I/O tasks poll every 10 ms independently of terminal
traffic. GNSS uses its existing 8 KiB DMA ring; companion RX has a 2 KiB interrupt
ring. Loss counters reset framing before post-loss data. No fix, stale fix,
transport silence and transport failure remain distinct observations.

Latest snapshots are copied under short locks. Readers calculate freshness from
observation timestamps; missed publications do not create a backlog. The input
queue holds 16 edges. Overflow records loss and delivers cancellation so a
consumer cannot retain a pressed state indefinitely.

BLE supports one peer. Core queues four bounded packets, each carrying connection,
sequence, receipt time and drop information. SDK clears continuity after detected
loss or reconnect. Trouble Host retains its upstream two-entry notification
queue; lag hidden inside that queue is not fully reported by the core drop
counter. This is not a lossless sensor stream or simultaneous-sensors claim.


Wi-Fi offers `wifi::initialize(peripheral, spawner).await -> Option<Stack<'static>>`
for consumers that need real DNS/TCP/UDP operations. It consumes the HAL Wi-Fi
token once, starts the existing connection/network/probe tasks and returns the
actual Embassy stack. None means unconfigured or radio initialization failed;
Some means transport resources exist, not link, DHCP or internet readiness.
Current `wifi::start` calls that interface and passes the returned handle to the
network-time consumer. Composition can instead retain the handle for another
same-executor consumer. No global getter or unsafe Send implementation is used:
Embassy Stack contains a RefCell reference and is Copy but not Send/Sync.

The stack handle survives reconnects. Consumers wait for config readiness with
a deadline, bound their DNS/connect/read/write waits, and cancel by dropping the
operation/socket. `connection_generation()` exposes the same association counter
used by the existing recovery checks; results tied to a link must check it after
awaiting. The four socket slots remain shared with DHCP, DNS, the public probe
and time synchronization. Additional clients must coordinate that bounded
capacity; the capability does not promise an unlimited socket pool. Existing
probe/SNTP deadlines and generation-scoped recovery remain unchanged.

The console loop yields through an Embassy 5 ms timer. It services general power
and optional recording before reading a terminal command, including when no
client is connected. Core acquisition has separate tasks. Flash and display
operations still block the caller and can delay other work; timer periods are
requested periods, not achieved latency guarantees.

The initial #55 proposal for four queued commands was revised to match this
ownership. Terminal has one pending reply and one transmission in progress.
It stops accepting another command while the pending reply is occupied. Replies
precede queued logs. Logs hold eight 384-byte records and count drops; terminal
has two 1,536-byte transmission buffers and a 1,024-byte payload formatter.
There is no storage request channel: the console exclusively borrows the store
for synchronous operations. SDK accepts one action and retains its result until
`RIDE STATUS` consumes it; another action is rejected while pending or unconsumed.

Display has one exclusive owner. RGB565 drawing retains the existing 80x106
mapping to the 240x320 panel; native solid fills need no full-frame allocation.
DMA completes before the transfer buffer is reused. The existing blocking HAL
wait has no new timeout or recovery guarantee for wedged hardware.

## Persistence and mutation completion

The device flash backend fixes application data at `0xd98000..0xe98000` and
settings at `0xe98000..0xe9a000`. Relative range, overflow and alignment checks
precede physical access. Core exposes data geometry and relative read/program/
erase operations without ride types. It provides arbitrary byte reads over the
physical word granularity. No API exposes stock, boot metadata or vendor writes.

The settings journal keeps its two sectors, `C606` magic, format version, commit
ordering and checksum. Valid older records survive torn updates. An occupied
journal without a valid supported record is an error, not empty space. A valid
journal containing unsupported or malformed preferences remains readable for
diagnosis but cannot be overwritten through settings save.

SDK retains ride slot bytes, commit ordering, catalog scan and recovery rules.
Each recorder iteration handles one scan sector, verified erase sector or
committed slot append. These steps can contain several physical reads/writes.
Settings save is one synchronous journal transaction. Flash can suspend interrupts;
transport loss detection remains required. Measured write/erase durations and
maximum counters are observations, not deadlines or guarantees against wedged
hardware. An ambiguous failure stops mutation; callers inspect/read back or
rescan rather than blindly retrying.

`RIDE INIT` requires the recorder's explicit initialization state. Clear requires
`RIDE CLEAR CONFIRM <upper>` and idle, nonpending recorder state with the same
occupied prefix bound. The host clear tool verifies a supplied export's SHA-256,
reads and hashes the full current prefix, checks unchanged export information,
then sends clear once. Verification, mutation and completion polling keep the
same no-reset USB descriptor and advisory lock. Timeout is an uncertain outcome.
There is no automatic format, reclaim or vendor filesystem write.

## Builds and terminal

Host base compilation uses no default features. `cycling` adds only SDK logic;
C606 builds select `c606`, optional `cycling`, and optional `debug-harness`.
Use `CYCLING_SDK=0|1` and `CYCLING_HARNESS=0|1` with `mise run build`. Ordinary
terminal and logging remain enabled in all four combinations. Harness builds
add explicit diagnostic fault/test hooks; they do not restore the removed UI.

Commands are `CMD <id> <command>` with a 128-byte line bound. Replies are JSON
Lines with `type`, `id`, `status`, device monotonic `ms` and textual `data`.
Core commands include `HELP`, `INFO`, `STATUS`, `POSITION`, `INPUT`, `BATTERY`,
`TIME`, `SETTINGS`, `BRIGHTNESS`, `TIMEZONE`, `IDLE`, `SAVE`, `ACTIVITY`, `WIFI`,
`BLE`, `STORAGE`, `DISPLAY` and `RESTART`. Wi-Fi/BLE support `RECONNECT`;
`TEST` controls supported build-gated diagnostics. Query help for argument forms.
Cycling adds `RIDE STATUS|SENSORS|HISTORY|START|PAUSE|RESUME|FINISH|INIT`, explicit
clear, and `EXPORT INFO|SLOT <index>`. Mutations return `ACCEPTED` and a token;
status polling reports completion. Export retains per-slot CRC-32 and host
SHA-256 identity verification.

## Porting and checks

A future board must supply concrete clocks/pins, memory initialization and DMA/
interrupt ownership, acquisition transports, USB, display/input capabilities,
and safe flash reservations/image limits. It must prove buffer lifetime,
transport loss behavior and hardware timing on that device. Retain read-only MMC
until separate write ownership is established. Never substitute generic flashing
for this repository's backup/flash/stock safeguards.

`check_boundaries.py` checks lower-layer imports and path attributes, ignoring
comments and literals. It is a source guard, not a Rust semantic analyzer.
Base and cycling tests/Clippy establish compile-time feature isolation; all four
C606 builds and observed device checks establish the supported hardware claims.
Publish code and findings. Keep device identifiers, credentials, vendor material
and raw capture evidence in ignored `.local/`.
