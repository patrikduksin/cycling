# Companion bootloader acquisition attempts

Date: 2026-11-06. Source checkout: `4ff4bb1de217` with the existing working-tree
research and update experiment. This follow-up tested two acquisition ideas:
find a bootloader in available stock artifacts, or find an ordinary memory-read
command in the stock companion application. All work ran on the host. There was
no device access, firmware change, bootloader-entry request, or external message.

**Result: neither route yielded a companion bootloader binary.** The available
packages contain companion applications and ESP32 applications. The inspected
companion UART handlers do not implement an address-and-length flash-read
operation. This narrows the search; it does not prove that no vendor factory
package or service mechanism exists.

## Stock artifacts inspected

The existing filesystem inventory contains 430 files and four application-update
packages. The read-only [unpacker](unpack.py) independently validated each
package's outer CRC, size and decoded payload. The two Nordic payloads identify
themselves as `NRF52810_APP`:

| Companion package | Decoded bytes | Application address range, end exclusive |
| --- | ---: | --- |
| N22 1.638 | 78,488 | `0x12000..0x25298` |
| N22 1.902 | 78,504 | `0x12000..0x252a8` |

Each decoded companion payload has one candidate Cortex-M vector table, at its
application start. Neither decoded ESP32 payload, N21 1.953 or N21 1.956, has a
matching Nordic vector table. No nested package header in the known encoded
format was found in any of the four decoded payloads.

The original 3,959,422,976-byte filesystem image and the 16,777,216-byte ESP32
flash backup were also scanned in full, read-only:

- The filesystem image has exactly four encoded package-header matches. Their
  decoded component labels, load addresses and payload lengths match the four
  inventoried files. No additional header was identified.
- Physical bytes after those headers do not form valid contiguous packages;
  filesystem fragmentation must be respected. Package integrity conclusions
  above come from the logically extracted files, not raw physical slicing.
- The ESP32 flash backup has no matching encoded package header.
- Neither raw image has the searched bootloader/component labels or a candidate
  aligned, uncompressed Nordic-sized Cortex-M vector table under the scan's
  criteria.

The vector scan requires a plausible aligned stack pointer within 24 KiB of
RAM, a Thumb reset address below `0x30000`, and plausible exception vectors with
at least five populated handlers. It is a heuristic. These scans cannot exclude
compressed/encrypted content, other container formats, atypical vector tables,
missing filesystem sectors, or packages absent from the backup. They did not
establish a complete companion-flash dump.

## Stock application's UART command surface

Analysis used the complete, hash-checked N22 1.902 decoded application, SHA-256
`e7e46db0b2f0f5be7a9fe4b10ef9996e15870f148fb4923df4833b9ee4dde6e5`.
Tracing the frame parser through the group/page dispatch identifies these paths:

| Group / page | Inspected behavior |
| --- | --- |
| Group `0`, page `0x53` | Converts date/time fields and updates the clock; replies with fixed-format fields. |
| Group `1`, page `0x01` | Returns fixed identity/version fields. |
| Group `1`, page `0x17` | Selects and configures/enables/disables ANT device profiles using numeric fields. |
| Group `1`, page `0x18` | Placeholder handler returning success without populating reply data. |
| Group `1`, page `0x24` | Application-mode query or scheduled DFU handoff, as already described in the recovery report. |
| Group `0x10`, page `0xe1` | One subtype dispatches to an ANT-related control routine; other subtypes perform no additional handler call. |
| Group `0x10`, page `0xe2` | Subtypes select button/event, power/control/settings, and ANT-related operations. |
| Groups `0x0b`, `0x11`, `0x22`, `0x23`, `0x28`, `0x78..0x7b`, `0x80` | ANT transmit forwarding with a fixed-format acknowledgment. |
| Other groups/pages | Unsupported/default handling. |

The paths inspected construct replies from constants, incoming fields and known
state. None offers a caller-selected flash address and length whose contents are
returned over UART. The ANT transmit path supplies packet bytes to a SoftDevice
service; it does not interpret those bytes as a local memory pointer.

An additional offline Unicorn execution exercised **7,655 dispatch inputs**:
all 256 group values; all 256 page values for groups `0`, `1`, `0x10`; all 256
subtype values for group `0x10` pages `0xe1`/`0xe2`; each frame class `1..5`.
Other payload bytes were zero. Execution stopped at each leaf handler, whose
behavior was inspected separately. The observed leaves matched the static map.
This checks routing coverage, not arbitrary payloads, leaf-function execution,
hardware behavior, exploitability, or every possible firmware input. No claim
of a comprehensive absence of memory-disclosure vulnerabilities is made.

## Public acquisition leads and next boundary

A bounded public search did not produce a companion factory/bootloader package.
The independent [HighwayStar/c606-oss project][upstream] was inspected directly:
its complete 183-entry repository tree and `main/nrf_link.c` expose no bootloader
binary or flash-read command in that link implementation. The captured Git tree
was `ec9f1ee6ef88bda847091ef94180399831e63f87`; the downloaded link implementation
matched its tree entry's blob hash. This is another implementation, not evidence
of every command supported by Magene's firmware.

The remaining useful acquisition evidence would be an actual companion factory
image, a verified service/readout mechanism, or physical debug access. Retrieving
memory by replacing the companion application would first cross the same
unproven-recovery boundary this investigation was intended to resolve. It was
not attempted. The [closed-case recovery conclusion](companion-recovery-sources.md)
therefore remains unchanged.

Private scripts, exact package headers, scans, source captures, disassembly and
emulation output are in ignored `.local/bootloader-search/`. Reproduction uses
`artifacts.py`, `vectors.py`, `disassemble.py` and `dispatch.py` there; Capstone,
NumPy and Unicorn run through the flake shell and `uv`. Vendor bytes remain in
the existing private artifact locations.

[upstream]: https://github.com/HighwayStar/c606-oss
