# Ride recording

Ride workflows belong to the optional `cycling` SDK. Build with
`CYCLING_SDK=1`; they work through the ordinary USB terminal with either
`CYCLING_HARNESS=0` or `1`. Base firmware leaves the ride reservation untouched
and rejects SDK commands with INVALID. There is no graphical Rides/History
page, injection session or terminal demo-start command.

See [architecture](architecture.md) for ownership and
[terminal/device testing](device-debugging.md) for framing, private capture and
safe device access. This document describes implemented behavior; it does not
claim that each path has been newly validated on hardware.

## Commands and completion

Requests use the ordinary `CMD <request-id> <command>` protocol. For example,
`CMD 1 RIDE STATUS` inspects the recorder without modifying flash.

| Command | Behavior |
| --- | --- |
| `RIDE STATUS` | Recorder state, used-slot bound, ride/sample/drop counts and pending/completed operation token |
| `RIDE SENSORS` | Selected sensor profile, link, fresh values/ages and decoder counters |
| `RIDE HISTORY` | Up to four latest completed summaries |
| `RIDE START` | Start a live ride when ready and capacity permits |
| `RIDE PAUSE` | Pause a recording ride |
| `RIDE RESUME` | Resume a paused ride |
| `RIDE FINISH` | Finish a paused ride |
| `RIDE INIT` | Explicitly initialize the owned reservation when state is `needs_init` |
| `EXPORT INFO`, `EXPORT SLOT index` | Read export metadata or one bounded slot |
| `RIDE CLEAR CONFIRM upper` | Destructive clear; use the hash-verifying host helper below |

Mutations return ACCEPTED with an operation token. That acknowledges acceptance,
not flash completion. Poll `RIDE STATUS` for the matching `completed` token and
`result=OK` or `FAILED` before sending another mutation. One operation can be
pending, and its completion is retained until a status response retrieves it.
Further mutations return BUSY while either is outstanding. A timed-out or lost
reply can leave completion uncertain; do not repeat the mutation blindly.

The console calls the SDK's service step independently of terminal requests.
GNSS, physical input and BLE transport have their own acquisition tasks. The
recorder performs at most one bounded media step per invocation; flash still
blocks its caller. No display loop or active graphical screen is required.

## Reservation, format and recovery

The owned ride reservation is 1 MiB at `0x00d98000..0x00e98000`, with the upper
address excluded. The two-sector settings journal starts at `0x00e98000`.
The safe application limit is `0x638000` bytes from the slot-B base. Ride writes
cannot reach stock slot A, the bootloader, partition table, settings journal,
eFuses or vendor bulk storage. Use repository flash/stock tasks; do not replace
them with generic flash commands.

Startup scans one 4 KiB sector per recorder service step. START does not erase
space. Unknown occupied data without a valid ride record yields `needs_init`;
initialization is an explicit destructive operation on this reservation, never
a boot default. Scan, write or erase failures stop recording for inspection.

The existing version-1 format remains 256-byte slots. Each holds a header and
up to four 48-byte samples, protected by a record CRC. A separate aligned word
is programmed last to commit the slot. Headers retain ride ID, sequence, active
duration and live/demo provenance. Old demo records remain readable; new terminal
rides are live and contain no generated demo speed.

Sampling targets one sample per second and writes batches of four. Pause and
finish freeze active duration at the accepted monotonic timestamp before flushing
pending samples. Delayed service lowers the achieved sample rate. Missed periods are not
backfilled or comprehensively counted; the drop counter covers full buffers and
samples discarded at capacity. A reset can lose the uncommitted batch: up to three
buffered samples between commits, or four while a commit is in flight.

On restart, a valid open ride is finalized with a recovery record at its last
committed active duration. Downtime is not added. Invalid occupied slots are
skipped and mark a gap. An uncertain write stops until reboot, when scanning
determines whether its commit reached flash. The last slot is reserved for a
FINISH, RECOVERED or FULL terminal record.

There are 4,096 slots. At one-hertz sampling and four samples per slot, capacity
is at most roughly 4.5 hours before event records, terminal-slot reservation and
partial batches reduce it. `RIDE STATUS` reports the used-slot bound; the complete
raw export is the inventory. HISTORY retains four summaries, not every old ride.
There is no automatic deletion, compaction or reuse when full.

## Sample semantics

Samples can contain system UTC milliseconds, fresh GPS coordinates, battery
percentage, heart rate and cadence. Live speed and distance remain unavailable.
Location-free recordings are valid; an indoor receiver reporting no fix is not
a reason to fabricate coordinates. Satellite/accuracy/elevation metadata are
absent from this record format and are not inferred during export.

HRS/CSC interpretation belongs to the SDK. Heart rate and cadence older than
five seconds, disconnected readings and measurements invalidated by detected
transport discontinuity are omitted. Heart-rate contact loss clears the usable
value. Core BLE selects one explicit peer/UUID pair; the current SDK selects one
HRS or CSC profile. Simultaneous heart rate and cadence from separate devices
are unsupported. Real readings and peer identifiers remain private.

Battery sampling retains the last observed percentage, including a stale core
observation; the ride format has no battery-age field. The ordinary BATTERY
command exposes freshness separately. A recorded percentage is therefore not
a guarantee of a fresh or calibrated battery-capacity measurement.

## Export

```sh
mise run ride-export
```

The host uses ordinary terminal requests with correlated IDs under the shared
USB lock. Export does not start a test session or modify flash. It is refused
while scanning, formatting, recording, paused, finalizing recovery or in an error
state. Wait for a known exportable state first.

`EXPORT INFO` reports format version, slot size, current upper bound and recorder
state. Each `EXPORT SLOT` reply contains the requested index, exactly 256 bytes
as 512 hexadecimal characters and a transport CRC. The host checks each index,
length and CRC, then rechecks metadata after downloading the prefix. A changing
or unavailable prefix fails the transfer.

The host writes `ride-slots.bin.partial` and renames it only after the complete
transfer passes those checks. It independently validates slot version, commit
word, record CRC, sequence, source and field flags. Invalid occupied slots remain
in the canonical raw prefix and are identified in the manifest rather than
silently discarded. An interrupted transfer leaves a partial file; a new export
starts at slot zero.

The export directory contains the raw prefix, its SHA-256, a manifest and one
JSON file per reconstructed ride. Ride IDs restart after clearing a reservation,
so identities also retain START-slot digests and the final raw digest.

GPX 1.1 contains only recorded location samples. It preserves recorded system UTC
when available and omits missing times. Pause/resume, invalid records, missing
locations, sequence gaps and backward UTC split segments. Longitude +180 is
normalized to -180; other invalid coordinates are omitted. A location-free ride
still exports raw/JSON with an explicit GPX-unavailable reason, not an invented
route. Keep exports and USB captures in ignored `.local/`, normally
`.local/exports/`.

## Explicit reclaim after verified export

First retain a complete export and its manifest. Then pass that exact directory:

```sh
mise run ride-clear -- .local/exports/<export-id>
```

The helper verifies the local raw length and SHA-256, reads the entire current
device prefix and requires an identical SHA-256, then rechecks metadata and the
upper-slot bound. It holds the same USB lock from verification through the
mutation and completion query. Only then does it send `RIDE CLEAR CONFIRM upper`.
The firmware validates that bound and its closed/exportable recorder state;
the host performs the full-content hash comparison.

Clear erases and read-verifies one sector per recorder step, confined to the
owned ride reservation. Success is reported only after all 256 sectors have
been verified erased. A completed scan containing only invalid remnants can be
cleared through the same fresh-export/hash-match workflow. Settings, MMC and
other flash regions are outside this operation.

A power interruption can leave a later portion of the old journal intact;
clear is not atomic deletion. On failure or timeout, do not resend automatically.
Restart, wait for scanning, inspect RIDE STATUS and export the remaining prefix
again before deciding whether to clear it. A complete clear resets ride IDs to
1; retained exports still identify content by hashes and START records.

## Historical evidence

September 11, 2026 tests used the now-retired graphical firmware. Two complete
exports matched a 22-slot, 5,632-byte prefix containing four rides. A later
explicit reclaim test verified a 30-slot prefix before clearing, then exported
an empty journal and a new four-sample demo ride. Preferences were preserved.
These are historical format/export/reclaim observations, not new terminal or
current-build hardware results.

Historical initialization measured up to 41 ms per sector and added GPS UART
errors during that run; normal tested appends measured 0–1 ms. Later UART/DMA
fixes and their separate transport evidence must not be conflated with those
older capture windows. Torn-body/commit, corruption, exact-full, recovery and
interrupted-clear behavior also have host fake-media tests; they do not prove
physical power-cut behavior at every flash instruction.

Current device state, observed validation and remaining hardware limits belong
in [the execution handoff](overnight-plan.md), rather than a historical ride
count or build claim in this guide.
