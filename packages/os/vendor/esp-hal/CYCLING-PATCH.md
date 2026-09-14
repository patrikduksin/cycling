# Local esp-hal patch

This directory contains the published esp-hal 1.1.2 crate from crates.io,
upstream commit `210eac8d035a991d452179464a684f853ff4be78`, under its original
Apache-2.0 OR MIT license. The crates.io archive omitted the repository-root
license files, so `LICENSE-APACHE` and `LICENSE-MIT` were copied byte-for-byte
from that exact upstream commit. Their Git blob hashes are respectively
`15c1d15b3a1ca9fbf1d24820fc487b998a6d2461` and
`b815dcb7b55a8561c29179dc4f7fc63d9984b47b`. The commit is the full 40-character
SHA recorded in the archive's `.cargo_vcs_info.json` and resolves unchanged in
the upstream repository.

The ESP32-S3-only `Rtc::sleep_light_with_status` entry clears stale RTC sleep
interrupts before requesting sleep, waits for wake or rejection under an RTC
deadline, and captures the raw cause and elapsed RTC time before cleanup.
The upstream `sleep_light` method returns no outcome and its ESP32-S3 path
immediately clears completion status. The public `wakeup_cause` helper requires
a deep-sleep reset, so it cannot establish the outcome of light sleep.

The additional entry retains CPU, digital peripherals, RTC RAM, main RAM,
flash/PSRAM supply, XTAL and PLL. It does not change the running CPU frequency
or disable caches. Existing upstream entry methods are unchanged. The caller
must quiesce DMA/radios and prevent concurrent interrupts/task switching; this
entry does not stop a second running CPU or compensate HAL/RTOS uptime.

No completion within the deadline cancels sleep admission and disables the
armed sources, but reports timeout rather than claiming entry or safe recovery.
The device keeps work gated for that outcome. Only hardware validation can
establish successful entry, wake and recovery on C606.

The local changes are confined to `src/rtc_cntl/mod.rs` and
`src/rtc_cntl/sleep/esp32s3.rs`. `cycling-sleep.patch` records the diff against
the published source for review and removal when upstream provides an equivalent.
