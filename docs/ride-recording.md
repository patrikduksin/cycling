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

Hardware initialization of the previously occupied reservation took at most
41 ms per sector and increased GPS UART errors by 19 during the one-time erase.
Normal committed appends measured 0–1 ms in the tested demo/live sessions and
did not increase GPS UART or companion error counters. These observations do
not prove interruption behavior at every flash instruction; host tests cover
torn bodies, torn commit words, corruption, invalid occupied space, exact-full
capacity and recovery from the last committed duration.
