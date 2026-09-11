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

The on-device Rides page keeps its deterministic demo controls. Physical starts
there create demo records. `RIDE START LIVE` is currently a harness-only start;
the page labels it `LIVE RIDE`, shows elapsed time, and displays speed and
distance as unavailable. A physical live/demo selector is follow-up work.

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

Hardware initialization of the previously occupied reservation took at most
41 ms per sector and increased GPS UART errors by 19 during the one-time erase.
Normal committed appends measured 0–1 ms in the tested demo/live sessions and
did not increase GPS UART or companion error counters. These observations do
not prove interruption behavior at every flash instruction; host tests cover
torn bodies, torn commit words, corruption, invalid occupied space, exact-full
capacity and recovery from the last committed duration.
