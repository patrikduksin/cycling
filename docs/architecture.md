# Architecture

Shells and apps consume device capabilities. Device implementations provide those
capabilities through component drivers and an MCU HAL. Embassy provides execution,
time, synchronization and reusable protocol stacks. A firmware entry point
composes a device and shell into a device-specific image.

Firmware belongs in `packages/os`; device research belongs in
`packages/stock/<device>`. Code owns API names, buffer sizes, timings and concrete
crate layout. GitHub issues and PRs own requirements, decisions and history.

## Ownership and dependencies

| Part | Owns |
| --- | --- |
| Capability interfaces | Separate contracts for display, input, storage, power, positioning and connectivity, including availability, limits, errors and completion semantics. |
| Component drivers | Component commands and protocols, using standard embedded interfaces where suitable. |
| Device implementation | Board wiring, MCU HAL resources, initialization, interrupts, buses, DMA, memory placement, power sequencing and hardware quirks. Implements the capabilities. |
| Shell and apps | Presentation, navigation, physical-input mapping, app lifecycle, preferences and domain policy. A shell manages access among its apps. |
| Firmware entry point | Selects a device and shell, allocates their resources and starts the required tasks. |

Shells depend on capability interfaces. Device implementations implement those
interfaces and use component drivers and the MCU HAL. Interfaces, drivers and
device implementations cannot import shell, app, domain SDK or diagnostic policy,
including behind feature gates. Only composition connects a concrete device to
a shell. Shell code cannot import a device implementation or branch on board names.

Shared mechanisms such as journals, protocol decoders and bounded queues belong
in focused reusable libraries. They do not own shell policy or form a central
System object or mandatory service registry. Each resource has one owner, either
through direct ownership or a device task with bounded request and observation
handles. Acquisition lifetime is independent of the foreground app and terminal
connection.

Use Embassy's executor, timers, synchronization and network stack directly.
Device code selects and initializes the compatible MCU HAL/runtime. Reuse
`embedded-hal`, `embedded-hal-async`, `embedded-io` and `embedded-io-async`
where they fit. Specialized buses and DMA can use device-specific adapters.
An adapter hides ownership or translates between interfaces; a wrapper that only
forwards calls adds another interface to maintain.

## Capability contracts

Capabilities are independently composable. A shell declares required capabilities
and resource needs, and handles optional capabilities explicitly. Missing hardware
must not report success or masquerade as a temporary failure. Build composition
checks required support where possible; initialization reports unavailable or
failed hardware. Device descriptions expose limits such as display geometry,
input controls and available storage. The same shell source runs on devices that
satisfy its capability and resource requirements.

Display exposes geometry, supported formats and submission completion. Device
code owns panel quirks and DMA lifetime; the shell owns layout, rendering scale
and presentation. Shared interfaces contain no board-specific geometry or rendering
policy. Buffer ownership lasts until DMA completes, including error and
cancellation paths.

Input exposes physical controls and observations without navigation or gesture
policy. Device code translates hardware reports into supported physical events
without inventing releases that the hardware cannot establish. Overflow cancels
held state. The shell maps controls to actions and chooses wake-input behavior.

Power exposes battery observations, brightness control and supported power
operations. Device code enforces hardware limits and safe sequencing. The shell
chooses inactivity timeouts, brightness preferences and user-facing sleep policy.
Shell preferences and their schema belong above device storage; board calibration
and hardware configuration remain device concerns.

Storage exposes checked relative access to owned reservations and explicit
completion/error behavior, without domain record types or access to stock and
vendor regions. Shared journal implementations can be reused. The shell or domain
library owns its schema, compatibility, recovery and export/reclaim semantics.
Unsupported or corrupt occupied journals remain available for diagnosis and
cannot silently become empty settings. Domain storage rules are described in
[ride recording](ride-recording.md).

Networking exposes the actual Embassy stack; connection-control contracts hide
hardware differences where needed. A stack handle means transport resources exist,
not that the link or internet is ready. It survives reconnects; callers bound
waits, check readiness and reject results from an obsolete connection generation.
Socket capacity is shared among consumers.

BLE and ANT transports expose profile-independent data with connection and loss
information. Device code handles controller and bridge quirks; reusable transport
code manages connections and channels. Domain consumers own sensor selection,
profile interpretation and continuity after loss or reconnect. A transport must
not promise lossless delivery when upstream queues can hide lag.

## Resource and observation semantics

Observations carry timestamps and loss/connection information where relevant.
Readers determine freshness for their use; no fix, stale data, silence and
transport failure remain distinct. Latest snapshots do not accumulate missed
publications. Bounded streams report loss; UART loss resets framing before
post-loss bytes.

A queued mutation is not a completed mutation. Ambiguous failures require
inspection or rescan before another attempt. One owner serializes terminal
replies and bounded logs, prioritizing replies. A missing host cannot block
acquisition or grow memory without bound. Blocking display and flash operations
mean timer periods do not guarantee achieved latency.

Device code owns memory-region, alignment and cache constraints. Capability
contracts express usable buffers and ownership without exposing those hardware
details to the shell. Device implementations preserve those constraints through
operation completion, failure and cancellation.
