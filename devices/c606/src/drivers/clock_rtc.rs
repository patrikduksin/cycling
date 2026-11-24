//! The device clock's copy in RTC-fast memory, for the C606.
//!
//! RTC memory and the RTC timer survive the resets of the digital core and
//! CPU alone: software, panic, main and RTC watchdogs on the core or CPU,
//! USB and deep sleep. Every other reset may have restarted the timer, so
//! boot drops the copy after one, whatever the counter now reads.

use core::ptr::addr_of;
use core::ptr::addr_of_mut;
use core::ptr::read_volatile;
use core::ptr::write_volatile;
use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::rtc_cntl::SocResetReason;
use firmware_services::network_time::{self, Clock, RETAINED_WORDS, Retained};

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RETAINED: [u32; RETAINED_WORDS] = [0; RETAINED_WORDS];

static KEPT: AtomicU32 = AtomicU32::new(0);

/// The RTC timer's count since power-on and the slow clock's calibrated
/// period, read as esp-hal's `Rtc::time_since_power_up` reads them. Raw
/// ticks, because the period changes with each boot's calibration.
fn rtc() -> (u64, u32) {
    critical_section::with(|_| {
        // SAFETY: the critical section keeps the sleep driver, the only other
        // user of these registers, out while the counter is latched and read.
        let rtc = unsafe { &*esp32s3::RTC_CNTL::ptr() };
        let delay = esp_hal::delay::Delay::new();
        // The latch takes up to one slow-clock period to settle.
        for _ in 0..10 {
            rtc.time_update().write(|w| w.time_update().set_bit());
            delay.delay_micros(1);
        }
        let high = u64::from(rtc.time_high0().read().timer_value0_high().bits());
        let low = u64::from(rtc.time_low0().read().timer_value0_low().bits());
        // esp-hal keeps the calibration in STORE1 (`rtc_slow_cal_period`).
        (high << 32 | low, rtc.store1().read().bits())
    })
}

fn timer_survived_reset() -> bool {
    matches!(
        esp_hal::system::reset_reason(),
        Some(
            SocResetReason::CoreSw
                | SocResetReason::CpuSw
                | SocResetReason::CoreMwdt0
                | SocResetReason::CoreMwdt1
                | SocResetReason::CpuMwdt0
                | SocResetReason::CpuMwdt1
                | SocResetReason::CoreRtcWdt
                | SocResetReason::CpuRtcWdt
                | SocResetReason::CoreUsbUart
                | SocResetReason::CoreUsbJtag
                | SocResetReason::CoreDeepSleep
        )
    )
}

/// Resume the clock the last run kept, before any source can speak.
pub fn restore(now_ms: u64) {
    let mut words = [0; RETAINED_WORDS];
    let source = addr_of!(RETAINED).cast::<u32>();
    for (index, word) in words.iter_mut().enumerate() {
        // SAFETY: index < RETAINED_WORDS; boot runs this before `keep` writes.
        *word = unsafe { read_volatile(source.add(index)) };
    }
    let (ticks, period) = rtc();
    let kept = timer_survived_reset()
        .then(|| Retained::decode(&words))
        .flatten();
    match kept.and_then(|kept| kept.resume(ticks, period, now_ms)) {
        Some(anchor) => {
            network_time::resume(anchor);
            log::info!(target: "clock",
                "CYCLING_CLOCK resumed unix_ms={} age_ms={} source={}",
                anchor.unix_ms, anchor.age_ms, anchor.source.name()
            );
        }
        None => {
            // A copy refused now must stay refused after a later reset that
            // keeps the timer, when the restarted count may have passed it.
            let target = addr_of_mut!(RETAINED).cast::<u32>();
            // SAFETY: the checksum word is inside RETAINED; boot runs this
            // before `keep` writes.
            unsafe { write_volatile(target.add(RETAINED_WORDS - 1), 0) };
            log::info!(target: "clock", "CYCLING_CLOCK none_kept");
        }
    }
    KEPT.store(network_time::generation(), Ordering::Relaxed);
}

pub fn keep(now_ms: u64) {
    let generation = network_time::generation();
    if KEPT.swap(generation, Ordering::Relaxed) == generation {
        return;
    }
    let Clock::Anchored(anchor) = network_time::clock() else {
        return;
    };
    let anchor = anchor.at(now_ms);
    let (rtc_ticks, _) = rtc();
    let words = Retained {
        unix_ms: anchor.unix_ms,
        age_ms: anchor.age_ms,
        source: anchor.source,
        rtc_ticks,
    }
    .encode();
    let target = addr_of_mut!(RETAINED).cast::<u32>();
    // SAFETY: every offset is below RETAINED_WORDS; the main task is the only
    // writer.
    unsafe {
        write_volatile(target.add(RETAINED_WORDS - 1), 0);
        for (index, word) in words[..RETAINED_WORDS - 1].iter().enumerate() {
            write_volatile(target.add(index), *word);
        }
        write_volatile(target.add(RETAINED_WORDS - 1), words[RETAINED_WORDS - 1]);
    }
}
