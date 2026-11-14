# esp-hal fork

Cargo's root `[patch.crates-io]` takes `esp-hal` and `xtensa-lx-rt` from
[this pinned fork commit](https://github.com/patrikduksin/esp-hal/commit/3dc1a1d25071acb0993a79cd4d1083e1253cdea0)
on branch `c606-light-sleep-esp-hal-v1.2.2-asm-order`. The branch is the esp-hal
1.2.2 release (upstream tag `esp-hal-v1.2.2`, commit `915465df59aee91b627fe538f0b442e56c7f4fc9`)
plus three commits:

1. the bounded light sleep below (`22a9b7da`, the tip of `c606-light-sleep-esp-hal-v1.2.2`);
2. a cherry-pick of upstream esp-rs/esp-hal#6497 (`8a70d09f`), for rustc 1.99 and later;
3. registry dependencies in `xtensa-lx-rt/Cargo.toml`.

The fork keeps the upstream MIT/Apache-2.0 licenses. Its `xtensa-lx-rt` is
the crates.io 0.23.0 release, which esp-hal 1.2.2 depends on, plus #6497.

## Startup code on rustc 1.99 and later (#6497)

rustc 1.99 no longer emits a module's `global_asm!` blocks in source order.
Without the fix, `__pre_init` fails to link ("l32r: literal placed after use"),
and xtensa-lx-rt's `Reset` ends after `.data` init, so it never sets VECBASE or
calls `__post_init` and `main`. The fix keeps each sequence in one block, in
`esp-hal/src/soc/mod.rs` and `xtensa-lx-rt/src/lib.rs`. Upstream merged it on
2026-10-15, after esp-hal 1.2.2 and xtensa-lx-rt 0.23.0. Drop the cherry-pick
when the next esp-hal release includes it, and drop the `xtensa-lx-rt` override
when its next release does.

## Bounded light sleep: why

esp-hal 1.2.2 has a `LowPower` driver whose `sleep_light` returns nothing. It
reports neither a rejected request nor one that never completes, and waits for
the hardware with no deadline. The device must know whether its timed sleep
happened before it resumes peripheral work. Upstream `main` returns only
`Ended` or `Refused` and still has no deadline. Keep the esp-hal override until
an upstream release supplies the behavior below.

## Bounded light sleep: what remains

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

The fork also changes `esp-hal/Cargo.toml` and `xtensa-lx-rt/Cargo.toml`: it
replaces the sibling-crate path dependencies with their registry versions.
Without that, a git dependency brings second copies of those crates from the
checkout; for `esp-rom-sys`, Cargo rejects two packages that link `esp_rom_sys`.

## Caller obligations and limits

- The caller arms a wakeup source first, and must quiesce DMA and radios and
  prevent concurrent interrupts and task switching.
- Unlike `sleep_light`, the entry does not suspend the UARTs or advance the
  system timer by the time slept. It does not stop a second running CPU.
- Only hardware validation can establish successful entry, wake and recovery on
  C606.
