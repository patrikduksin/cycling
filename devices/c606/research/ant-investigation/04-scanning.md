# 4. Discovery while sensors remain connected

## Result

**Concurrent discovery is substantially more plausible than the previous notes
suggested.** The recovered scan implementation reserves its own channels and does
not close the sensor channels. The current disconnect-before-scan policy is ours;
it is not established as a companion requirement.

## Evidence

The existing scan request reaches N22 1.902 handler `0x1472a`, then `0x13344`.
It checks only the states of discovery channels 0 and 10 before proceeding. The
generic scan selects a wildcard channel identity and opens channel 0, with an
event path at `0x1330c` alternating the discovery channels. Their configurations
are at `0x24260` and `0x24274`, initialized by `0x1c128`.

The two channels search different ANT networks. In the configurations above,
channel 0 selects network index 0 and channel 10 selects index 1. Both select
RF channel 57, or 2457 MHz, and extended assignment value 1 for background search.
The type-128 branch in `0x13344` selects the second channel, identifying its Di2
role. General discovery alternates the channels as each search closes. An ANT
network is a protocol configuration, not another physical radio.

The public request handler ignores its type-selector byte and calls the scanner
with type zero. Consequently, the recovered private type-specific scan modes
should not be advertised as host-selectable features.

Scan stop at `0x13530` closes channels 0 and 10 and sends a scan-ended report. It
does not invoke the all-sensor disconnect sequence at `0x131fc`. The timer created
at `0x1bb94` also uses this stop routine.

The path calls ordinary channel-open SVC `0xc4`; it does not call receive-scan-mode
SVC `0xc6`. This distinction matters because Nordic's dedicated
[receive scanning mode](https://docs.nordicsemi.com/r/bundle/s212_v2.0.0_api/page/group_ant_interface.html?contentId=wvkAChoVg_C0~McERr5qmA)
requires other channels to be closed. That restriction should not automatically
be applied to this different implementation.

Stock N21's scan wrapper at `0x421ab44c` constructs the same request as our adapter.
The wrapper itself has no sensor-disconnect operation. Its callers were not
exhaustively traced.

## Remaining uncertainty and decision

Background search can still compete for radio time. The installed unit has not
been tested here for data continuity during discovery. Moreover, the scan wrapper
at `0x1472a` ignores the scan routine's return value and constructs a positive
reply, so that reply cannot prove discovery started successfully.

**Make concurrent discovery the first controlled transport experiment.** Preserve
the current receive baseline, start one bounded scan, and compare per-sensor
packet timing, losses, discovery results and stop behavior. Do not equate continued
UART activity with uninterrupted sensor reception. No such scan was sent during
this research session.
