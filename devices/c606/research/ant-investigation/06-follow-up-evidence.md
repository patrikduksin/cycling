# Follow-up evidence and remaining hardware work

Research date: 2026-09-15. This supplements reports 1, 2, 4 and 5.
No firmware implementation changed.

## Offline execution

Executed N22 1.902 instructions using Unicorn 2.1.4 in Thumb mode. Flash was mapped
at the recovered base; scratch RAM and a return sentinel were supplied. The
actual type lookup and wrapper instructions ran. ANT SVCs returned controlled
values; logging calls were skipped and bridge serialization was intercepted.
This was a function-level experiment, not a simulation of the complete radio stack.

| Experiment | Observation |
| --- | --- |
| TX wrapper `0x135cc`, all ten supported types, eight-byte payload, each of four radio return values | 40/40 runs copied the first two request bytes, set reply byte 2 to 1, and returned reply length 8. |
| Radio return values | `0`, `1`, `8`, `0xffffffff`; the wrapper did not distinguish them. Nonzero values exercise error handling without assuming release-specific error meanings. |
| Event handler `0x1be78`, power channel, events 5 and 6 | Neither execution called bridge serialization. |
| Same handler, events 1 and `0x3c` | Both called bridge serialization, providing positive controls. |

Private reproduction script and full results are in ignored
`.local/ant-research/verify_bridge.py` and `emulation-results.json`.

## Static checks

An exhaustive aligned halfword search of the decoded N22 1.902 application found:

| ANT API operation | SVC | Occurrences |
| --- | --- | --- |
| Acknowledged TX | `0xc8` | One, at `0x16f2e`, in the recovered TX function. |
| Broadcast TX | `0xc7` | None. |
| Burst TX | `0xc9` | None. |
| Version query | `0xeb` | None. |
| Capabilities query | `0xec` | None. |

These are the opcodes in the matched vendor ABI. Absence establishes that no
direct call with that encoding exists in the application image. It does not
prove what the uncollected SoftDevice or bootloader contains.

The radio observer at `0x184a8` calls the common event handler, then `0x141fc`.
The latter maintains speed reception statistics; it does not encode a correlated
TX outcome. The generic incoming-data forwarder at `0x132a8` collapses broadcast,
acknowledged and burst message IDs into the same eight-byte sensor report.

## Why two discovery channels

The structures at `0x24260` and `0x24274` specify channels 0 and 10, respectively.
Both use bidirectional slave assignment, background-search extension value 1,
RF channel 57, wildcard identity and initial period zero. Network indices differ,
0 and 1. The assignment helper at `0x18340` passes the network index to SVC `0xc2`.
The type-128 branch in `0x13344` selects the second search path; the firmware labels
this profile Di2. The generic mode alternates both paths through `0x1330c`.

The public scan handler at `0x1472a` always passes type zero. Dedicated type
selection inside the scanner therefore does not imply a host-selectable option.
Network keys remain private and are not reproduced here.

## Stock TX and restricted configuration

N21 `0x421b0b34` queues a sensor type and at most eight data bytes. The power
calibration sender `0x421b0b68` chooses type 11, page 1, then calls encoder
`0x421b0bcc`. Its ID `0xaa` branch fills the six remaining bytes with `0xff`.
This recovers the payload `01 aa ff ff ff ff ff ff` independently of guessing
from a profile name. Sending it would perform calibration, so it was not sent.

N21 `0x421acc9c`, labeled `AntPwrPpThresholdSet`, sends a different eight-byte
power payload through the same helper. It must not be confused with calibration.

N22 group 16, page `0xe2`, subcommand `0x0c`, handler `0x17804`, accepts a restricted
power-period setting when payload byte 5 is zero. Byte 6 values 4 and 8 select
periods 8182 and 4091 respectively. The helper `0x1b404` invokes channel-period
SVC `0xd1` for channel 3. This changes local receive scheduling; the code does
not send an instruction that changes the remote sensor's broadcast rate.

## What needs hardware next

| Question | Evidence needed |
| --- | --- |
| Fitted chip on our unit | FCC internal photos identify nRF52810-QFAA on the certification board, PCB V5.0.2. Establish its relationship to our board revision, or use read-only companion debug-register access. |
| Exact installed binaries | Companion flash readback, including stack region, compared with reference images. |
| More than four active types | Five distinct owned sensor types, with per-sensor reception observations. |
| Scan while connected | Bounded scan with reception timing and loss measurements for existing peers. |
| Successful outgoing request | A diagnostic sender and an owned sensor returning the requested page; distinguish an expected response from a periodic broadcast. |

A request-page response can confirm one profile operation. The bridge's positive
reply cannot establish radio acceptance, and a timeout cannot distinguish a lost
request from a lost response. Do not blindly retry commands that change a sensor.

The current console has no arbitrary ANT TX command. These physical experiments
belong to the next implementation/test stage. No new flash, reset, setting change
or ANT transmission was needed for this follow-up.
