# Guarded companion marker trial

## Outcome, 2026-11-07: the loader does not start a modified image

The fixed one-byte candidate (identity minor `0x13` -> `0xa3`) was submitted
twice more, from a guard that holds a custom identity until the host records
it. Both runs gave the same answer:

| Boot | Companion traffic | Outcome |
| --- | --- | --- |
| Before (stock) | 10 frames by 2 s; identity at 4.9 s | Trial submitted once |
| 1, during transfer | 614 packets, zero retries | ESP32 power-cycled near the last packet |
| 2, after the custom image | **Zero bytes for 30 s** (5 s in the first run) | Guard restored stock |
| 3, after restore | 10 frames by 2 s; stock identity at 4.9 s | Healthy |

A running N22 application sends reports unprompted within two seconds of every
boot. Silence while the loader still accepts DFU entry and a full stock image
means the **loader kept itself in DFU and did not start the patched
application**. The patch touches no startup code, so a crash-before-UART is
much less likely than rejection; a crash loop would also keep cycling ESP32
power, which boot 2 did not show.

The first 2026-11-07 run used the earlier 5 s identity deadline, which stock
itself barely meets (4.9 s), so the guard now waits 30 s. The 2026-11-06 run
below lost its evidence the same way.

### The likely check

Every N22 image carries a descriptor at offset `0x400`: magic `a5 5a 55 aa`,
a 32-bit word that differs per build (`0x7a116739` in 1.638, `0x622cc565`
in 1.902), then flash start `0x12000` and the image size. The vendor container
header is a copy of this descriptor, and its XOR key is derived from that
word. The application never references its own descriptor (no literal
`0x12400`), so the loader is its consumer. Our patch left the word unchanged.

The word is not, for both builds at once: CRC-32 (IEEE, Castagnoli, BZIP2,
MPEG-2, POSIX, JAMCRC; every refin/refout/xorout combination), 32-bit or
byte sums, XOR32, Adler-32, or the first/any 4 bytes of MD5, SHA-1, SHA-256,
SHA-512 or BLAKE2s. Ranges tried: the whole image; from `0x400`, `0x408`,
`0x420`, `0x480`, `0x1000`; the field zeroed, `ff`-filled or skipped; the
descriptor skipped; padded to 1 KiB/4 KiB; and the encoded container payload.
The earlier CRC-16 search against the container header found nothing either.

### Stock ESP32 firmware does not use the field

The N21 1.956 main firmware was read where it handles this package format
(private disassembly, addresses only):

- The companion sender's `UpdateInit` (`0x421b2568`) checks the container
  CRC-16/XMODEM, finds the decoded magic, copies header bytes 4..127 into its
  module (+32..), and derives the payload XOR key from header bytes 6..7
  (`0x421b2970..299f`). Nothing reads module +32/+33, header bytes 4..5, in
  the updater region (`0x421b2400..4200`).
- The ESP's own self-update (`0x421b3488`, "Start Master Update") runs the
  same `UpdateInit`, then `esp_ota_begin/write/end/set_boot_partition`. Its
  integrity check is ESP-IDF's image check, not this field.

So header bytes 4..5 are written by the vendor's packaging tool, and only the
companion loader can consume them. They are not **any** CRC-16 (all 32,768
odd polynomials; init 0/`ffff`; both bit orders; xorout 0/`ffff`) of either
N21 payload, decoded, fixed-key-only or as stored, nor of either N22 image
from 0, `0x408` or `0x480` with the field kept, zeroed or `ff`-filled. They are
not a catalogue CRC-16/32 or sum over header-plus-payload with the field
blanked. With four samples, a plain checksum is ruled out: the field is either
a random nonce (bytes 6..7 beside it are the XOR key) or a keyed value.

### Control trials, 2026-11-07

Two more guarded runs settle what the loader accepts:

| Candidate | After transfer | Verdict |
| --- | --- | --- |
| 1.902 with one build-date character changed (`0x47b`, `4` -> `5`) | Zero companion bytes for 30 s; guard restored stock | **Rejected** |
| Unmodified vendor N22 **1.638** (older) | 10 frames by 2 s, GNSS flowing, identity reply at 1.7 s in its own format (`... 26 10`) | **Accepted and ran**; restored to 1.902 on acknowledgment |

The date string is data no code reads, so the loader checks the **whole
image** against something carried inside it, and an intact older vendor image
passes (downgrades are allowed). The check is self-contained in the image the
ESP sends; the per-build descriptor word at `0x404` is the only candidate
found, and its algorithm is not a plain CRC-16 or a catalogue CRC-32/sum/hash.

The first date run was misread by the observer as accepted: startup readiness
answered BUSY to `TEST 43` during the silent boot, hiding the restore. Trial
reads are now admitted during startup, and "runs as stock" requires having
watched the post-transfer probe. The boot logs show the rejection plainly.

### Next evidence needed

Custom companion code needs the loader's integrity algorithm. Remaining routes:
an algebraic search for an arbitrary-polynomial CRC-32 at `0x404` (two images
constrain the polynomial through a GF(2) GCD), or reading the loader over SWD.
If the word is a keyed MAC, only the loader (SWD) reveals the key.

Evidence: private `.local/companion-trial/1794010608897906019`,
`1794010825912652651`, `1794012484401439188` (date) and `1794012725493810399`
(1.638) in the owner's checkout.

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
