# Logging and private capture

The base and cycling compositions use the Rust `log` facade, `serde-json-core`
and standard JSON Lines. A fixed queue feeds the same nonblocking USB owner as
ordinary terminal replies. No hosted backend, custom decoder or new runtime is
required. The small Linux tty adapter supplies the repository's no-reset lock,
reconnect handling and bounded line assembly; Python's JSON decoder does the
record decoding. Domain events belong to the emitting service or SDK.

## Options checked

| Option | Verified fit | Gap for this device |
| --- | --- | --- |
| `log` with `esp-println` | Locked `log` 0.4.34 supports structured key/value records; `esp-println` 0.18.0 supports ESP32-S3 USB Serial/JTAG and a `log-04` backend. No alternate executor required. | Stock backend formats synchronously, waits in USB output, and does not report discarded bytes. Use the facade with a bounded sink instead. |
| `defmt` with `esp-println` and espflash | `defmt-espflash` supplies rzCOBS framing that can coexist with ordinary text. `defmt` 1.1.1 is already locked; espflash 4.5.0 is pinned. | Same underlying synchronous printer. Requires matching ELF metadata for decoding and transport integration for the existing terminal owner. This feature combination has not been built here. |
| `defmt` over RTT with probe-rs | RTT is an established RAM-channel transport. `probe-rs attach` documents attachment without reset or flashing. | Current firmware has no RTT logger. C606 JTAG attachment and coexistence with this USB workflow are unverified. Target-family support alone does not establish safe board access. |
| pySerial/miniterm | pySerial supports timeouts, pre-open DTR/RTS state and POSIX exclusive access. `device.py monitor` already uses pySerial with both control lines false. | miniterm is an interactive terminal; it does not provide cycling boot/sequence summaries or participate in `.local/usb.lock` automatically. Extend existing host code instead of adding another competing reader. |
| Existing crash tooling | Locked `esp-backtrace` 0.20.0 provides the installed panic path; `crash_rtc.rs` records a versioned marker and resets through custom hooks. | Backtrace text is best effort. RTC retention is not durable flash storage, and host logs cannot recover events that never arrived. |

Official references: [log 0.4.34](https://docs.rs/log/0.4.34/log/),
[esp-println 0.18.0](https://docs.espressif.com/projects/rust/esp-println/0.18.0/esp_println/index.html),
[defmt encoding](https://defmt.ferrous-systems.com/encoding),
[espflash 4.5.0 monitor source](https://github.com/esp-rs/espflash/blob/v4.5.0/espflash/src/cli/monitor/mod.rs),
[probe-rs attach](https://probe.rs/docs/tools/probe-rs/),
[pySerial API](https://pyserial.readthedocs.io/en/latest/pyserial_api.html),
[miniterm](https://pyserial.readthedocs.io/en/latest/tools.html).

The pinned Rust crates use MIT/Apache-2.0 licensing; pySerial uses BSD licensing,
as described in its [license documentation](https://pyserial.readthedocs.io/en/latest/appendix.html).
All collection is local and offline. Recheck licenses for any new
serializer or host dependency before adding it.

## Device bounds

Pinned `esp-println` 0.18.0 can wait 50,000 iterations on a full USB FIFO inside
a critical section, then silently discard bytes. It remains only in early board
startup and the best-effort panic/backtrace path. Ordinary service logs use the
bounded sink; no service task waits for a reader. This does not promise bounded
panic output or startup before the executor starts.

There are eight 384-byte record slots. Sequence numbers wrap at u32; cumulative
queue/encoding loss saturates at u32::MAX. Each attempt takes a sequence before
enqueue. Full queues drop the new record; encoding failures drop the entire
record. The next successfully enqueued record includes `lost`. The facade keeps
at most 160 UTF-8 bytes of formatted message and labels truncated messages. A
random per-boot token distinguishes resets without exposing a device identifier.

The one USB owner retains partial records and offers at most 64 bytes per nominal
5 ms poll using HAL `write_byte_nb`/`flush_tx_nb`. Replies have priority between
records and cannot interrupt log bytes. It holds one pending reply plus the
record being sent, each bounded to 1,536 bytes. A slow reader delays replies and
loses queued logs, but cannot block acquisition. Successful enqueue/TX is not a
promise that a host received the bytes. Gaps/truncation on the host remain visible.

`STATUS` reports log attempts, total/max measured enqueue+serialization microseconds,
loss/depth, target queue storage bytes, and maximum measured active USB pump time.
The enqueue measurement excludes facade message formatting; the USB pump includes
reply traffic. Instrumentation itself has a cost. Report firmware configuration
and collection state with measurements; these are observed samples, not worst-case
latency, peak memory or power measurements. See [the final validation evidence](refactor-validation.md) for
actual values. No screenshot or recording buffer remains in the base.

Boot records contain commit/dirty state, harness/SDK mode, recording=false, queue
limits and INFO level. Reset/crash markers, service transitions, storage results,
periodic progress and free-heap samples use the same queue. Default service logs
exclude credentials, identifiers, coordinates, sensor readings and vendor bytes.
The generic facade cannot identify secrets inside an arbitrary formatted string.

## Collect and inspect

```sh
mise run logs -- --seconds 60 --output .local/logs/session
mise run logs -- --seconds 5 --command STATUS --command POSITION
mise run logs -- --filter .local/logs/session/records.jsonl --component position
mise run logs -- --filter .local/logs/session/records.jsonl --level WARN
```

`terminal` and `logs` share this single owner. Each new collector session
writes private `raw.bin`, `records.jsonl` and `summary.json` under ignored `.local/`.
The interactive/one-shot terminal stores its raw traffic separately in `.local/terminal/`.
Raw bytes are saved before decoding. `host_ns` is host receipt UTC time; device
`ms` is monotonic. The line accumulator is bounded to 4,096 bytes, discards overlong
input until newline and records incomplete lines at disconnect/end. ROM/panic
text remains raw with `unstructured` markers. Host logs cannot recover records
that never reached the computer.

The recorder holds `.local/usb.lock` throughout reconnect attempts. It uses
`os.open`, `tty.cfmakeraw` and `tcsetattr(TCSANOW)`, clears HUPCL, and performs no
DTR/RTS ioctl or input flush. pySerial's open path issues modem-control ioctls and
TCIFLUSH, and `tty.setraw` defaults to TCSAFLUSH; neither is used here. Opening
retries use a two-second cap. File failures stop collection visibly. The lock
excludes cooperating tools, not arbitrary external programs.

Each connection automatically sends read-only `INFO`, so late attachment gets
build metadata. User `--command` requests have session-local IDs and are sent only
once, never replayed after reconnect. A timeout or disconnect leaves a mutation's
completion unknown. A reconnect is not necessarily a reboot; a changed boot token
is a separate boundary. `RESTART` tries to drain the reply for 100 ms, with a 1-second
fallback that can reset before a slow host receives the acknowledgment.

If the device is silent immediately after a safe flash, the documented
`mise run monitor -- --seconds 5` control-line release can start/reset it. Stop
that task before opening the no-reset recorder. Final evidence records observed
startup failures separately from successful attachment; do not hide them.

Synthetic sanitized record:

```json
{"type":"log","boot":42,"seq":7,"ms":2200,"level":"WARN","component":"position","event":"transport_recovery","message":"uart_errors=1","lost":0}
```

`TEST 10` intentionally saturates the queue; `TEST 11` requests a controlled panic.
`TEST 20` stalls the executor for 6 seconds to exercise transport buffering/recovery.
These commands need the harness and are used on base firmware for fault checks.
They do not establish electrical power control, physical input accuracy or panel
output. Host parser/queue tests cover partial input, dropped records, boot changes,
overlong input and sequence wrap. Hardware acceptance requires the measured
session evidence; test success alone is insufficient.
