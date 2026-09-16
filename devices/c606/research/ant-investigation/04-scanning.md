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

## Follow-up: explicit stop leaves the timer scheduled

Offline inspection for [#118](https://github.com/patrikduksin/cycling/issues/118)
found a second source of scan-ended reports. In N22 1.902, a zero-duration request
reaches `0x13344` and calls `0x13530` directly. That stop routine clears the scan
flags, closes channels 0 and 10, and emits a scan-ended report unconditionally.
It does not call the timer-stop routine at `0x18f3c`. Already-cleared flags do not
suppress another report.

Initialization at `0x1bb94` creates the timer with callback `0x13531` and mode zero.
The timer-start implementation at `0x18ee4` supplies a zero repeat interval for
this mode, establishing a single-shot timer. Scan start clamps the requested
seconds to at least five, then passes that value plus one to `0x13570`. The latter
converts seconds to timer ticks. Timer initialization at `0x18e88` selects RTC
prescaler one through `0x21c64`, giving 16,384 ticks per second under Nordic's
[RTC prescaler definition](https://docs.nordicsemi.com/r/bundle/ps_nrf52810/page/rtc.html).

A function-level Unicorn check executed the recovered timer conversion with
inputs 6, 11 and 61 seconds. The intercepted timer-start calls received 98,304,
180,224 and 999,424 ticks respectively. A separate stop-routine check with scan
flags already zero observed both channel-close calls and one serialized end
report. Radio calls and serialization were intercepted; no physical radio or
live timer scheduler ran. The reproduction tools and firmware remain private in
ignored `.local/` storage.

The recovered paths therefore permit an explicit-stop report followed by a timer
report for the same scan. They also permit the old timer to close discovery
channels belonging to a newer scan if the host starts it too early. The timer
setup's conditional cancellation checks a flag that explicit stop clears; it is
not evidence that a subsequent scan reliably cancels the old timer.

Keep ownership of the old scan until its outstanding stop and timer reports have
settled. An early explicit-stop report alone does not establish that its timer is
finished. Timeout or UART loss cannot establish quiescence either. Actual
installed timer scheduling, duplicate report timing and UART delay remain
unverified on hardware; the recovered timer interval is not a measured delivery
bound.
