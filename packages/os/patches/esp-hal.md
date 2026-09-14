# esp-hal sleep patch

Cargo's root `[patch.crates-io]` uses [this pinned fork commit](https://github.com/patrikduksin/esp-hal/commit/0cf3835559c18f03d75190b332316ba85b68b8ed) of esp-hal 1.1.2, based on upstream commit `210eac8d035a991d452179464a684f853ff4be78`.
Upstream MIT/Apache-2.0 licenses remain in the fork. The fork keeps the published
registry dependencies instead of pulling sibling crates from its Git checkout.

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

The hardware implementation changes only `esp-hal/src/rtc_cntl/mod.rs` and
`esp-hal/src/rtc_cntl/sleep/esp32s3.rs`. All HAL sources, linker files, build script
and configuration match the previously vendored and hardware-tested copy.
Remove the override when an upstream release supplies the required behavior.
