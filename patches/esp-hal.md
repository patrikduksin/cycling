# esp-hal sleep patch

Cargo's root `[patch.crates-io]` uses [this pinned fork commit](https://github.com/patrikduksin/esp-hal/commit/0cf3835559c18f03d75190b332316ba85b68b8ed)
of esp-hal 1.1.2, based on upstream commit `210eac8d035a991d452179464a684f853ff4be78`.
The fork keeps the upstream MIT/Apache-2.0 licenses and the published registry
dependencies rather than sibling crates from its Git checkout. Remove the override
when an upstream release supplies the required behavior.

## Why

Upstream `sleep_light` returns no outcome, and its ESP32-S3 path immediately
clears completion status. The public `wakeup_cause` helper requires a deep-sleep
reset, so it cannot establish the outcome of light sleep.

## What changes

The ESP32-S3-only `Rtc::sleep_light_with_status` entry:

- clears stale RTC sleep interrupts before requesting sleep;
- waits for wake or rejection under an RTC deadline;
- captures the raw cause and elapsed RTC time before cleanup;
- on deadline expiry, cancels sleep admission, disables the armed sources and
  reports timeout rather than claiming entry or safe recovery. The device keeps
  work gated for that outcome.

It retains CPU, digital peripherals, RTC RAM, main RAM, flash/PSRAM supply, XTAL
and PLL, and does not change the running CPU frequency or disable caches.

The sleep behavior changes only in `esp-hal/src/rtc_cntl/mod.rs` and
`esp-hal/src/rtc_cntl/sleep/esp32s3.rs`. Apart from those, the fork changes only
`esp-hal/Cargo.toml`, replacing sibling-crate path dependencies with registry
ones. Existing upstream entry methods, other HAL sources, linker files, build
script and configuration match the previously vendored and hardware-tested copy.

## Caller obligations and limits

- The caller must quiesce DMA and radios and prevent concurrent interrupts and
  task switching.
- The entry does not stop a second running CPU or compensate HAL/RTOS uptime.
- Only hardware validation can establish successful entry, wake and recovery on C606.
