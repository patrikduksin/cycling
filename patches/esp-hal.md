# esp-hal sleep patch

Cargo's root `[patch.crates-io]` uses [this pinned fork commit](https://github.com/patrikduksin/esp-hal/commit/22a9b7dac5a2d02646f5b84ee7aae9357afa7451)
on branch `c606-light-sleep-esp-hal-v1.2.2`. The branch is the esp-hal 1.2.2
release (upstream tag `esp-hal-v1.2.2`, commit `915465df59aee91b627fe538f0b442e56c7f4fc9`)
plus one commit. The fork keeps the upstream MIT/Apache-2.0 licenses. Remove the
override when an upstream release supplies the behavior below.

## Why

esp-hal 1.2.2 has a `LowPower` driver whose `sleep_light` returns nothing. It
reports neither a rejected request nor one that never completes, and waits for
the hardware with no deadline. The device must know whether its timed sleep
happened before it resumes peripheral work. Upstream `main` returns only
`Ended` or `Refused` and still has no deadline.

## What remains

`LowPower::sleep_light_with_status(config, timeout)`, in
`esp-hal/src/rtc_cntl/sleep/mod.rs`, for the ESP32-S3 only:

- clears stale sleep wakeup and reject interrupts before the request;
- waits for a wakeup or a rejection under an RTC deadline;
- on deadline expiry, cancels sleep admission and reports `TimedOut`, which is
  not proof that the chip did not sleep;
- returns `Woke`, `Rejected`, `TimedOut` or `NoWakeupSource` as a
  `LightSleepOutcome`.

Everything else comes from 1.2.2. The entry reuses its wakeup-enable mask,
source hooks, `RtcSleepConfig::apply`, `start_sleep` and `finish_sleep`. It
records the wake the way `sleep_light` does, so upstream `wakeup_cause` names
the source. The firmware arms the timer with `set_wakeup_deadline`, builds the
retained light-sleep configuration from the public `RtcSleepConfig` setters, and
measures elapsed RTC time with `Rtc::time_since_power_up`
(`devices/c606/src/drivers/sleep.rs`).

The fork also changes `esp-hal/Cargo.toml`: it replaces the sibling-crate path
dependencies with their registry versions. Without that, the git dependency
brings its own `esp-rom-sys`, and Cargo rejects two packages that link
`esp_rom_sys`.

## Caller obligations and limits

- The caller arms a wakeup source first, and must quiesce DMA and radios and
  prevent concurrent interrupts and task switching.
- Unlike `sleep_light`, the entry does not suspend the UARTs or advance the
  system timer by the time slept. It does not stop a second running CPU.
- Only hardware validation can establish successful entry, wake and recovery on
  C606.
