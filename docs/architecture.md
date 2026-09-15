# Architecture

A reader should be able to predict where behavior lives before searching for it.
The repository follows the things we build: a cycling application, shared firmware
packages, firmware for each device, and development tools. Package boundaries
express ownership. They should reduce what a caller needs to understand.

This document explains design intent. Code owns concrete interfaces; Cargo, mise
and CI own build and verification mechanics. GitHub issues and PRs own requirements,
decisions and history. Change those authoritative sources instead of copying their
details into this document.

## Where to look

| Path | Owns |
| --- | --- |
| `apps/vana` (`vana`) | Cycling behavior and its presentation: sensor selection and profiles, workout screens, ride state, metrics, recording, recovery, export and app preferences. |
| `devices/c606` (`c606-firmware`) | C606 hardware and the firmware assembled for it. Drivers, wiring, HAL resources, interrupts, DMA, memory placement, startup and board quirks live here. Sanitized device research lives in `research/`. |
| `packages/device-api` (`device-api`) | Hardware-independent capability contracts and the observations needed to use them. |
| `packages/shell` (`firmware-shell`) | Foreground presentation, physical-input routing, shared rendering, preferences and inactivity policy. |
| `packages/services` (`firmware-services`) | Portable acquisition, transport and persistence mechanisms with no board, presentation or cycling policy. |
| `packages/console` (`firmware-console`) | Device command sessions, framing, bounded replies and logs, and command integration. |
| `tools/simulator` (`shell-simulator`) | Host composition and simulated capabilities, including controllable input, time and failures. |
| `tools/devtools` | Host workflows for backup, flashing, terminal access, exports and test scenarios. |
| `vendor` | Redistributable dependencies that require a maintained local patch, including Trouble Host. |
| `.local` | Ignored vendor images, dumps, disassembly, credentials, identifiers and raw captures. |

A device package owns its hardware implementation and final firmware composition.
Its entry point chooses the application, allocates resources and starts tasks.
Keep application scheduling and policy with the application or shell that owns
them; startup should make the assembly visible.

## Give each package a meaningful job

A package earns its boundary by owning behavior, resources or a contract that
callers can use without learning its internals. File count and line count do not
decide package boundaries.

Keep VANA's domain logic and screens together while they serve one application.
Separate their responsibilities inside the package. Extract a domain library when
a real consumer needs it and the interface is clear. Keep simulated devices with
the simulator until another consumer needs them. Component drivers can stay
private to the device that uses them.

`services` holds portable mechanisms, such as decoding a positioning stream or
maintaining a journal. Its modules remain independent. It must not become a
central owner of all state or a home for code whose ownership is unclear. A
mechanism can become its own package when its consumers and lifecycle justify
that boundary.

A useful check is to imagine deleting the module. If its complexity spreads into
callers, the module earns its place. If only forwarding code disappears, the
extra interface probably does not help.

## Make paths explain behavior

Name files after the behavior a reader seeks. `ride/recorder.rs` and
`drivers/display.rs` give more direction than `manager.rs` or `utils.rs`. Put
technical details under the concept that owns them. Split a growing file when its
responsibilities have useful names, and create a directory when those
responsibilities belong together. Empty symmetry makes navigation harder.

Each concept has one source of truth. Remove old paths and compatibility modules
when a repository-wide move updates all callers. Keep package entry points small
and deliberate; they describe the supported interface, not every internal file.
Keep tests beside their owner. Shared scenarios belong with the tooling that runs
them; hardware-specific checks belong with the device workflow.

Package documentation should explain ownership, the main entry points and how to
find verification commands. It should help someone start a change without
repeating implementation details that will drift.

## Dependencies follow ownership

Shells and applications consume `device-api`. Device implementations provide those
contracts, using portable services, component drivers and an MCU HAL. The console
integrates commands through explicit handlers. Application-specific commands keep
their policy in the application.

The constraints are deliberate:

- Capability contracts cannot import their implementations.
- Hardware implementations and portable services cannot import shell or app policy,
  including behind feature gates.
- The shell cannot import a concrete device or branch on board names.
- The shell provides input routing and presentation operations. VANA owns its
  runtime; composition calls its lifecycle methods directly. A generic app trait
  is unnecessary until a real consumer needs it.
- Only firmware or simulator composition connects concrete devices and applications.

A device package can contain both a hardware library and a composition binary.
The binary's dependencies do not give hardware modules permission to import app
policy. Keep that distinction explicit in imports and review, as Cargo package
dependencies alone cannot enforce boundaries between modules.

Use Embassy's executor, time, synchronization and network stack directly. Reuse
standard embedded interfaces where they fit. Read the pinned dependency's source
and interfaces before inventing an alternative. An adapter earns its place when
it hides setup, ownership or a real translation. Renaming each native operation
through another wrapper gives readers two interfaces to learn.

## Small interfaces must hide substantial work

A caller should request an operation or read an observation. It should not mutate
internal fields and then remember to save preferences, refresh a screen or restart
a task. Keep the invariant and the work needed to preserve it with one owner.

Capabilities are independently composable. A consumer asks for the capabilities
it uses; there is no mandatory global `System` object or service registry. Device
startup supplies independent handles. Missing hardware is explicit. It cannot
report success or pretend to be a temporary failure.

A small interface still needs an honest contract. State what is available, which
limits apply, who owns buffers, what completion means and how failure appears.
Use typed results and observations; terminal code owns their text formatting.

| Capability | Boundary and reason |
| --- | --- |
| Display | Expose geometry, supported formats and submission completion. The device owns panel quirks and DMA; the shell owns layout and rendering policy. Callers must know when a buffer is safe to reuse. |
| Input | Expose supported physical controls and observations. The shell maps them to actions and chooses wake behavior. Device code must not invent releases the hardware cannot establish; overflow cancels held state. |
| Power | Expose battery observations and supported brightness and power operations. The device enforces hardware limits and sequencing; the shell chooses preferences and inactivity policy. |
| Storage | Expose checked relative access to owned reservations. Callers never receive access to stock or vendor regions. Journal mechanics are reusable; the owner of the stored data chooses its schema and recovery rules. |
| Positioning | Expose timestamped observations with freshness and loss information. Keep byte framing, decoding and acquisition state behind the contract. Consumers decide whether a fix is useful for their task. |
| Network | Expose the actual Embassy stack with connection control where hardware needs it. Stack resources can exist while the link is down. Callers bound waits, check readiness and reject results from an obsolete connection generation. |
| BLE and ANT | Expose profile-independent transport data with connection and loss information. Devices handle controller quirks, services manage transport state, and VANA interprets profiles and chooses sensors. |

## Ownership includes time and failure

Each resource has one owner. It can provide direct access or bounded request and
observation handles to a task. Acquisition lifetime is independent of the
foreground screen and terminal connection. Losing a host connection cannot stop
recording or leave memory growing without bound.

Observations distinguish no fix, stale data, silence and transport failure.
Readers decide freshness for their use. Latest-value snapshots do not accumulate
missed publications. Bounded streams report loss, and UART loss resets framing
before post-loss bytes enter a decoder. A transport cannot promise lossless
delivery when an upstream queue can hide lag.

Acceptance into a queue does not mean a mutation completed. Expose completion and
failure so callers can make the next decision. After an ambiguous write failure,
inspect or rescan before retrying. Keep reply and log output bounded, with one
owner serializing output and prioritizing replies.

Memory-region, alignment and cache constraints belong to device code. Preserve
buffer ownership through DMA completion, failure and cancellation. Blocking flash
or display work can delay other tasks; a configured timer period alone does not
prove achieved latency. Validate changes to hardware access and display timing on
the device, and report the behavior actually observed.

## Persisted data belongs to its user

Shell preferences belong to the shell; cycling preferences and rides belong to
VANA; calibration belongs to the device. The current shell settings envelope also retains the BLE profile selector for
compatibility; VANA interprets that value. Moving its physical storage requires an
explicit format migration. Moving code does not authorize discarding
or reinterpreting their persisted data. Preserve compatibility or provide an
explicit migration with recovery behavior. A package move must also preserve
terminal commands, reply framing and binary record formats. Verify compatibility
through their existing tests; source relocation alone is not evidence that a
device still behaves correctly.

Unsupported or corrupt occupied journals must remain available for diagnosis.
They cannot silently become empty settings or free space. VANA owns ride recovery,
export and reclaim rules, described in [ride recording](ride-recording.md).
Storage mechanisms enforce their bounds without interpreting those domain rules.

Preserve stock firmware, bootloader, partition table, eFuses and vendor
filesystems. Device workflows own the exact backup and flashing procedure.
Publish our code, tools and sanitized findings. Keep vendor firmware, disassembly,
flash dumps, credentials, identifiers and raw captures in ignored `.local/`.
The redistributable Trouble Host source and its licensed patch belong in `vendor`;
retain the license and minimum-MTU regression coverage.
