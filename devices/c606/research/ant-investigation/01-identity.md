# 1. Installed companion identity

## Result

**The connected companion self-reports firmware version 1.902.** This is now
supported by a fresh query and the recovered stock formatting, rather than inferred
from an update filename. A user-supplied PCB photo now independently identifies
an nRF52810-QFAA on the photographed board. The installed binary hash remains
unknown; the photo has not been tied to the connected unit.

## Evidence

N22 1.902's query handler at `0x15eec` constructs a ten-byte identity reply. Payload
bytes 8 and 9 are respectively 2 and 19. Its startup report at `0x16f34` uses the
same version bytes. In N22 1.628, the corresponding values are 28 and 16.

N21's receiver at `0x4204b9ac` splits payload byte 9 into decimal quotient and
remainder. Its version formatting at `0x4202f207` uses quotient, remainder and
byte 8, with the last field padded to two digits. Thus the two image values become
1.902 and 1.628. This independently agrees with both release filenames.

The live read-only query completed at uptime 3,001,454 ms and returned the newer
image's version fields. The subsequent observation reported `reply_observed`,
fresh identity, zero invalid reports and zero recorded losses. Two retained
2026-09-14 observations agree. Raw replies remain private.

The first identity field is not a chip identifier. The startup serializer obtains
it through `0x1513c`, which consults a board-variant value derived from GPIO31 at
`0x15590`. The query handler instead hard-codes this field to zero. It therefore
cannot reliably identify even that board variant through a query.

## PCB photo evidence

On 2026-09-15, the user supplied a PCB photograph through the clipboard. The
lower-right radio chip visibly reads `N52810`, with package code `QFAA` on the
next line. Nordic's [reference circuitry documentation](https://docs.nordicsemi.com/r/bundle/ps_nrf52810/page/ref_circuitry.html?contentId=rrhhkNhja5o3MUK3azOE4w)
identifies nRF52810-QFAA as the QFN-48 variant. This independently corroborates
the update's NRF52810 build label with physical package-marking evidence.

The user then supplied the source PDF, the 11-page internal-photo document for
FCC ID `2ALZG-262`, publicly indexed as attachment 6914653. Page 8 contains the
clipboard photo; page 10 gives a closer view of the Nordic marking. Page 11
explicitly labels the adjacent PCB antenna as ANT+. Page 7 shows PCB revision
`V5.0.2`. This identifies nRF52810-QFAA as the ANT companion on the certification
board. It does not establish the fitted part on every C606 revision or read back
the connected unit's stack.

Source: [FCC internal photos, attachment 6914653](https://fcc.report/FCC-ID/2ALZG-262/6914653.pdf),
inspected from the user's local copy after the web download failed. The PDF,
clipboard image and provenance hashes remain in ignored `.local/ant-research/`.

## Remaining uncertainty and decision

The update's embedded application label names NRF52810, and the code is consistent
with that target. This is build-target evidence, not a physical part readback.
The main-processor flash backup does not include companion flash or its radio stack.
No fitted-chip or stack-version readout was found in the inspected query path.

**Proceed with N22 1.902 as the matching research reference.** Report the observed
version separately from silicon identity and binary verification. Exact chip
identification need not block experiments using already recovered bridge commands.

## Follow-up: where identification stops

The application starts at `0x12000`, and its RAM layout starts at `0x20000b80`.
Those boundaries match the vendor's [S212 7.0.1 release notes](https://www.thisisant.com/assets/resources/Release_Notes/ANT_s212_nrf52_7.0.1_releasenotes.pdf).
This supports the S212 family identification; memory boundaries are not a unique
release fingerprint. Do not label the installed stack 7.0.1 on this evidence.

An aligned instruction-byte search found no SVC `0xeb` or `0xec`, the version and
capabilities operations in the matching ANT ABI. The inspected host-command
dispatch has no general chip-register or flash-read command. Firmware references
to Nordic factory registers occur in local initialization, not identity replies.
The public C606 internal-photo filing was located, but its PDF could not be
retrieved from the available mirrors. The user subsequently supplied that PDF
locally, providing the physical identification documented above.

**Identity research has reached the limit of the available read-only interface.**
The usable conclusion is N22 1.902, targeting nRF52810, with an S212-compatible
ANT interface, with nRF52810-QFAA physically identified in the supplied photo.
Confirmation for the connected unit requires matching the photo to its board
or a companion debug-register read. Exact stack and application verification require
companion flash readback. The ESP32 backup and update package cannot provide it.
These checks require additional physical access; no companion reflash is needed
or justified merely to identify it.
