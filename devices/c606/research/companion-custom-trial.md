# Guarded companion marker trial

## Outcome, 2026-11-06

One stock-derived custom application was submitted to the connected companion.
The last captured transfer status was packet index 605 of 614, with zero retries
at that packet. USB disconnected, returned, and the main CPU restarted. The USB
observer then stopped on an id-zero `INVALID malformed command` response while
reconnecting. It never resent the trial start.

Later read-only checks captured a fresh checksum-valid stock identity response
(version bytes `02 13`), with the experimental guard in Ready state. Motion and
battery reports were fresh; companion CRC and UART error counters were zero.
GNSS transport and parsing were progressing without a position fix indoors.
Main STATUS reported a power reset and no crash.

**The custom marker was not captured.** Neither the timed restore state nor the
last transfer acknowledgment was captured. A further main power cycle and the
returned stock identity are consistent with the guard's automatic fallback, but
do not distinguish a rejected custom image from custom execution followed by
restoration. This is an inconclusive execution test, not proof of custom-image
acceptance or recovery from a broken companion application. No second custom
trial was submitted.

## Candidate and offline evidence

The candidate is generated from the pinned, independently unpacked N22 1.902
application. Exactly one instruction byte changes: the immediate returned by
its fixed identity response moves from `0x13` to `0xa3`. Instruction boundaries,
vectors, startup, image descriptor, power handling and DFU code are unchanged.
Both original and patched identity functions were executed in an offline ARM
emulator and returned the expected distinct response bytes. The bootloader's
image integrity policy is unknown; an unchanged descriptor does not establish
that a patched application is acceptable.

The build requires `CYCLING_COMPANION_STOCK`, checks the package and decoded
image hashes, and enables this fixed candidate only with
`CYCLING_COMPANION_TRIAL=1` and the diagnostic harness. It accepts no arbitrary
candidate file. All vendor bytes, instruction listings and raw evidence remain
in ignored `.local/companion-trial/`.

## Automatic stock restore mechanism

The experimental ESP32 image embeds the entire original companion application
as well as the candidate. Its guard owns companion UART traffic during probing,
transfer and restoration. It starts with fresh identity queries on every main
CPU boot. Stock identity releases ordinary traffic and allows one explicitly
submitted trial. Custom identity arms a 30-second observation interval before
stock restoration. Failure to obtain a recognized identity within five seconds
of initial probing also triggers one stock update attempt. An upload failure
triggers the same fallback.

After a successful transfer without an ESP32 reset, the guard observes for 30
seconds, attempts stock restoration and verifies stock identity. If the
companion power-cycles the ESP32, boot probing independently re-evaluates its
identity. A failed restore does not repeat indefinitely in the same boot.

This is best-effort stock reinstallation driven by the ESP32, not atomic A/B
rollback on the companion. It depends on a running ESP32, working UART and a
companion that still accepts update entry. It cannot force an unknown
bootloader to recover an application that disables those paths. Its deadlines
are relative to boot observation or transfer completion, not an unconditional
wall-clock rollback from the first flash packet.

## Verification and provenance

The device ran base revision `4ff4bb1de217` with uncommitted uploader, guard and
probe changes, SDK0/harness1. The candidate trial ran in main slot A through
`cycling device flash --companion-trial`; flash verification preserved ride and
settings reservations. The observer sends TEST44 at most once, including across
uncertain command completion and reconnection. Its failure on a read-only
console parse rejection was corrected afterward, with a regression test that
asserts the trial start is not replayed.

Before hardware access, Rust base and cycling tests passed (1,500 and 1,808
respectively), as did 84 additional host tests and 15 Trouble Host tests.
`cycling check`, the experimental build, 617-frame private reference comparison,
guard tests, and the full Python suite passed. The earlier full test invocation
had one incorrect observer fixture; it was corrected and the entire Python
suite rerun successfully before device access.

Final cleanup installed the ordinary SDK0/harness1 base in main slot B, with
both companion payload build variables unset. INFO confirmed
`cycling=false harness=true`; TEST43 returned Unsupported. At approximately
33 seconds uptime, motion and battery reports were fresh, GNSS transport was
receiving and parsing, companion CRC/UART counters were zero, and power control
was idle/ready. STATUS reported no crash. The main application was verified by
the normal flash workflow and ride/settings reservations remained unchanged.
All USB readers exited. After the observer correction, `cycling check` and the
full Python suite passed again (508 tests, 103 subtests, plus 12 additional
Python tests).

## Next evidence needed

An observer must survive main power cycles long enough to capture a fresh custom
identity and the later stock restore. If a future deliberate trial remains
unobserved, acceptance/integrity checks and the bootloader remain unresolved.
A successful marker trial would still not establish recovery from an arbitrary
crashing application. Standby current and motion wake have not been measured or
implemented by this trial.
