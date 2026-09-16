# 5. Outgoing commands, acknowledgments and bursts

## Result

**The existing bridge has a concrete outgoing-message path matching ANT
acknowledged transmission.** The obstacle is reliable completion reporting:
the bridge throws away the radio call's result, and its examined event handler
does not forward transmit-completed or transmit-failed events.

## Evidence

N22 1.902 dispatch `0x17b48` sends supported sensor-type groups through `0x13640`
and `0x135cc` to `0x16f0c`. This resolves the fixed type channel and calls SVC
`0xc8` with channel, payload length and payload pointer. N22 1.628 contains the
same call pattern at `0x16ee8`.

The vendor-authored legacy [ANT interface header](https://github.com/lab11/nrf5x-base/blob/c3dff2dbdeeb304f63cea3e2fcea245b6e5670a8/sdk/nrf51_sdk_10.0.0/components/softdevice/s210/headers/ant_interface.h)
maps `0xc8` to acknowledged-message transmission. Neighboring channel, identity,
status and enable calls also match. This is a strong ABI identification, not proof
of the exact installed SoftDevice version. Nordic documents the matching
[operation and eight-byte payload](https://docs.nordicsemi.com/r/bundle/s212_v2.0.0_api/page/group_ant_interface.html?contentId=wvkAChoVg_C0~McERr5qmA).

At `0x135cc`, the bridge discards the result and creates a reply containing the
first two request bytes and a positive flag. `0x17c30` sends it as class 5. This
proves only execution of the wrapper, not radio acceptance or peer reception.

The common radio event handler at `0x1be78` handles receive and connection events,
but events 5 and 6 take its default path. Those are TX-completed and TX-failed in
the matching vendor API. No corresponding bridge completion report was found.
No complete outgoing burst command path was found either. Recognizing incoming
burst message IDs does not establish a usable burst-transfer interface.

## Remaining uncertainty and decision

No outgoing ANT message was sent during this investigation. Installed behavior,
sensor compatibility and profile-level replies remain untested. Generic RF,
network and channel configuration remains internal to the companion; no general
host configuration interface was recovered.

**Start with a read-only profile request that produces an identifiable response
page from an owned sensor.** Bound it to eight bytes and one outstanding request.
Track bridge reply and sensor response separately. Use a timeout that reports
uncertain delivery; do not automatically replay calibration or control commands.
A profile response may prove a specific operation worked even if the bridge
cannot expose the radio acknowledgment. Reliable generic TX status and burst
support remain separate work.

## Follow-up: verified behavior and usable commands

Offline execution of the recovered wrapper verified 40 cases across all ten types:
successful and failed radio returns produced identical positive bridge replies.
TX completion/failure events emitted no bridge frames; connection-event controls
did. This tested actual wrapper instructions with a mocked radio.

Stock's power-calibration sender uses the same eight-byte path. Its recovered
payload is `01 aa ff ff ff ff ff ff`. This establishes a real stock use, but
calibration should not be the first transport test. A whole-image search found
one acknowledged-TX SVC and no direct broadcast or burst-TX SVC. Incoming burst
forwarding also loses the information required for generic reassembly.
See the [evidence appendix](06-follow-up-evidence.md) for addresses, checks and
a separate restricted local power-channel period setting.

**Host firmware can add profile-specific eight-byte commands with response-page
confirmation. It cannot recover generic radio delivery status from this bridge.**
True radio completion reporting would require a companion firmware change or a
newly discovered companion interface. Actual delivery, response correlation and
timeouts still require an owned sensor and a controlled hardware experiment.
No existing console command exposes arbitrary ANT TX, so that experiment needs a
small future diagnostic implementation. No implementation or RF transmission was
performed in this research pass.

## ANT+ payload coverage

Eight bytes is the normal ANT message payload, as defined by Garmin's
[ANT Message API](https://developer.garmin.com/connect-iq/api-docs/Toybox/Ant/Message.html).
ANT+ normally organizes that payload into data pages, usually with a page number
in the first byte. A feature may exchange several independent pages. See the
[vendor profile implementation guidance](https://thisisant.developer.garmin.com/pages/developer/ant-plus/developer/index.html).

The recovered send path therefore has the required payload size for ordinary
ANT+ command pages on its supported types. This does not establish complete
profile support: channel configuration, timing, response interpretation and sensor
support still matter. ANT-FS/file transfers and burst operations need additional
transport functionality. Repeated acknowledged pages are not a replacement for
burst sequencing.

## Reply ownership

The examined N22 dispatcher at `0x17c30` calls the sensor handler once and emits
one class-five reply at `0x17c64` before returning. The handler at `0x13640`
calls the wrapper at `0x135cc` once. No delayed reply, retry or timer path was
found for this command; the radio completion events described above emit none.
Unlike discovery stop, this send path has no second scheduled reply to drain.

The C606 adapter therefore releases a reply key after consuming its timely
matching bridge reply. Normal consumers can repeat the same command. A timeout,
partial write, transport loss or cancellation after UART submission retains the
key, so an unresolved old reply cannot complete a later request. Cancellation
before submission releases it. This relies on the examined firmware's one-reply
behavior; it does not promise protection from arbitrarily duplicated UART frames.
