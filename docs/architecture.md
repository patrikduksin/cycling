# Architecture

Firmware stays in `packages/os`. Device code hides board wiring, HAL resources
and hardware quirks, transferring each peripheral to one owner. Core provides
general capabilities such as acquisition, display, settings, connectivity and
bounded storage. The optional cycling SDK owns sensor interpretation and
selection, ride lifecycle, sampling, recovery, history and export/reclaim
semantics. Consumers compose these layers. Device and core cannot import SDK
or diagnostic policy, including behind feature gates.

Use Embassy's executor, timers, synchronization and network stack directly.
An adapter earns its place by hiding ownership or translating between layers;
a wrapper that only forwards calls adds another interface to maintain.

Acquisition runs independently of terminal traffic and consumers. Latest
snapshots carry observation times, so readers calculate freshness when reading
and missed publications do not accumulate. No fix, stale data, silence and
transport failure remain distinct. Input overflow cancels held state; UART
loss resets framing before post-loss bytes. BLE carries generic notification
bytes with connection/loss information; the SDK resets sensor continuity after
loss or reconnect. The upstream notification queue can hide lag, so the stream
is not lossless.

One owner serializes USB replies and bounded logs, prioritizing replies. A
missing host must not block acquisition or grow memory without bound. A queued
mutation is not a completed mutation; ambiguous failures require inspection or
rescan before another attempt. Display and flash operations can block the
caller, so timer periods do not guarantee achieved latency. Display ownership
lasts until DMA completes.

The Wi-Fi capability exposes the actual Embassy stack on the same executor.
A stack handle means transport resources exist, not that the link or internet
is ready. It survives reconnects; callers bound waits, check readiness and reject
results from an obsolete connection generation. Socket capacity is shared with
the built-in network consumers.

Core storage exposes checked relative access to owned reservations, without ride
types or access to stock and vendor regions. Settings retain their on-flash
format and commit ordering. Unsupported or corrupt occupied journals remain
available for diagnosis and cannot silently become empty settings. The SDK
owns ride format compatibility and recovery; export precedes explicit verified
reclaim. See [ride recording](ride-recording.md).

Normal allocation, radio/task state and DMA stay in internal memory. PSRAM has
a separate allocator and cannot hold atomics or data needed with its cache
disabled. New PSRAM DMA use requires alignment/cache validation on hardware.
The licensed Trouble Host patch remains necessary for minimum-MTU discovery.

Code owns API names, buffer sizes, timings and geometry. The boundary check is a
source guard, not a Rust semantic analyzer; both base and SDK compilation matter.
C606 is the verified board. [Research](../packages/stock/magene-c606) distinguishes
measured hardware from stock-supported driver candidates.
