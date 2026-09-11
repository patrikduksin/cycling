# Ride recording

The C606 stores rides in the 1 MiB owned reservation from `0x00d98000` through
`0x00e98000`. The settings journal remains at `0x00e98000`. The safe application
limit is therefore `0x638000` bytes from the slot-B base. Stock slot A, the
partition table, bootloader, settings journal and other vendor regions are not
written.

Startup scans one 4 KiB sector per display loop. Empty space is never erased by
`START`. If the reservation contains unknown data and no valid ride record, the
recorder reports `needs_init`; only the explicit harness `RIDE INIT` command
erases one reserved sector per loop and verifies it reads back erased. A scan or
erase error stops the recorder. Formatting does not run automatically.

Records are 256-byte version-1 slots. A CRC covers the header and up to four
48-byte samples, and a separate aligned word is programmed last to commit the
slot. Headers persist the ride ID, sequence, active duration and `demo` or `live`
provenance. Samples can carry UTC milliseconds, fresh GPS coordinates, battery,
and future heart/cadence values. Live samples omit demo speed. GPS satellites,
accuracy and BLE sensor values remain absent because they are not yet associated
with the recorded sample epoch or a continuous live sensor connection.

Recording samples once per second and writes batches of four. Pause and finish
freeze at the accepted monotonic timestamp before a pending batch is written.
After reset, a valid open ride is finalized with a recovery record at its last
committed active duration; time while the device was down is not added. Up to
one four-sample batch can be lost (three buffered between commits, or four while
a commit is in flight). Delayed sampling and explicit drop counters are separate.
Invalid occupied slots are skipped and
mark the open ride as having a gap. An uncertain write stops until reboot so the
scanner, rather than an automatic retry, decides whether the commit reached
flash. The last slot is reserved for `FINISH`, `RECOVERED`, or `FULL`.
The reservation holds 4,096 slots, at most about 4.5 hours of one-hertz samples
at four samples per batch before event and partial-batch overhead. There is no
reclaim, delete or reuse command yet; reaching full stops new recording. Safe
reclaim after export is tracked in [#44](https://github.com/patrikduksin/cycling/issues/44).

The on-device Rides page has a mode row while ready. Tapping it selects `DEMO`
or `LIVE`, and the right button starts that source; the left button still opens
History. The selected mode remains fixed through pause, resume and finish, while
the recorder's committed source is authoritative for an active ride. Live rides
show elapsed time and explicitly unavailable speed and distance. Debug sessions
can temporarily change the selection, but `END` restores the prior value.
On hardware, injected touch selected Live while preserving History navigation.
The ready view showed zero elapsed time and unavailable speed/distance; a
temporary Live ride advanced to 2.18 seconds with both values still unavailable.
Ending the session restored Demo and left the durable journal at four rides and
slot 22. These injected gestures verify firmware routing, not physical touch or
button operation. GPS UART errors increased by one during this capture window;
the unresolved transport loss remains tracked in
[#34](https://github.com/patrikduksin/cycling/issues/34).

The History page retains the four latest completed summaries in a fixed array
and shows two per page. Its header reports retained/total rides, so older rides
are not presented as deleted or browseable through this first bounded view.
Each summary shows source, saved/recovered/full/gap state, duration, deterministic
demo distance, first known sample date, GPS sample count, and average heart rate
or cadence when present. Missing values are explicit. The first known UTC sample
is not labeled as ride start time because active duration excludes pauses. Raw
storage remains the complete inventory for export.

On hardware, the bounded scan reconstructed four existing rides through slot 22
without writing the journal. Both two-ride pages rendered the saved/recovered and
live/demo labels plus first-known dates and explicit missing sensor fields. GPS
UART errors increased from 0 to 2 during the broader capture window and then
stayed at 2 in the follow-up state sample; this is retained as the existing GPS
transport issue rather than evidence that history capture is loss-free.
A later targeted two-page capture held rides/slot at 4/22, GPS UART at 0,
companion UART at 1, and free heap at 80,276 bytes while companion valid frames
advanced 393 to 421; observed maximum frame time rose from 25 to 32 ms.

## Export

`mise run ride-export` reads the complete occupied prefix without starting an
injection session or changing flash. `EXPORT INFO` captures format version,
256-byte slot size, current upper bound and recorder state. Each correlated
`EXPORT SLOT` response contains one exact index, 512 hexadecimal characters and
a transport CRC. Export is refused during scan, formatting, recording, pause,
recovery finalization or an error. A physical start during transfer causes the
next request to fail rather than exporting a changing ride.

The host writes `ride-slots.bin.partial` and renames it only after every slot
passes index, length and transport CRC checks. It then independently validates
record version, commit word, record CRC, sequence, source and field flags. The
canonical raw prefix and its SHA-256 remain alongside a manifest and one JSON
file per ride. An interrupted run leaves a partial file; retry starts from slot
zero. Ride IDs are local to a formatted reservation, so the manifest identity
also includes the START slot digest and final raw digest.

GPX output uses version 1.1 and only recorded location samples. It preserves
the recorded system UTC estimate when present and omits time when absent. Pause,
resume, invalid records, missing locations, sequence gaps and backward UTC split
track segments. Longitude +180 is normalized to -180; other invalid coordinates
are omitted. Location-free rides still export raw/JSON and report `no valid
location samples` rather than creating a route. Elevation, accuracy, and
satellite metadata remain absent. All exports and raw USB logs stay in ignored
`.local/exports/` unless an explicit output directory is selected.

On hardware, an intentionally interrupted transfer stopped after one 256-byte
slot and left only the partial file. Two subsequent complete transfers each
read the same 22-slot (5,632-byte) prefix and produced identical raw hashes.
Both recovered the existing four rides with 10/4/5/5 samples: one saved demo,
two recovered demos and one saved live ride. No slot failed integrity
validation. The live ride had no demo speed, and all four reported that GPX was
unavailable because those recordings contained no locations. Recorder state
remained ready at four rides and slot 22 before and after export; saved
brightness, dim and timezone values were also unchanged.

Hardware initialization of the previously occupied reservation took at most
41 ms per sector and increased GPS UART errors by 19 during the one-time erase.
Normal committed appends measured 0–1 ms in the tested demo/live sessions and
did not increase GPS UART or companion error counters. These observations do
not prove interruption behavior at every flash instruction; host tests cover
torn bodies, torn commit words, corruption, invalid occupied space, exact-full
capacity and recovery from the last committed duration.
