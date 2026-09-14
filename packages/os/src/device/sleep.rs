//! Bounded ESP32-S3 light sleep with RTC evidence and retained volatile memory.
//!
//! Call only after device work and DMA have quiesced. The C606 starts one CPU;
//! the critical section below prevents that CPU's scheduler and interrupt tasks
//! from touching peripherals during entry. This is not a second-core parking API.
//! HAL/Embassy uptime excludes sleep. Callers must invalidate acquisition stamps
//! and use `rtc_elapsed_us` when accounting for the real elapsed interval.
use core::cell::RefCell;
use critical_section::Mutex;
use cycling_os::capabilities::Error;
use esp_hal::{
    peripherals::LPWR,
    rtc_cntl::{Rtc, sleep::TimerWakeupSource},
    time::{Duration, Instant},
};

const SLEEP_SECONDS: u64 = 10;
const ENTRY_DEADLINE_SECONDS: u64 = 12;
const TIMER_WAKE_BIT: u32 = 1 << 3;

static RTC: Mutex<RefCell<Option<Rtc<'static>>>> = Mutex::new(RefCell::new(None));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Hardware latched the armed timer wake after a substantial RTC interval.
    TimerWake,
    /// The sleep controller explicitly rejected entry.
    Rejected,
    /// A wake event arrived too early to establish this ten-second sleep cycle.
    Immediate,
    /// No terminal hardware event was observed. Keep device work gated.
    TimedOut,
    /// Hardware returned evidence inconsistent with the sole armed wake source.
    UnexpectedWake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub outcome: Outcome,
    pub rtc_elapsed_us: u64,
    /// Uptime spent across the call, which excludes the sleep interval.
    pub uptime_elapsed_us: u64,
    pub wake_bits: u32,
    pub rejected: bool,
    pub woke: bool,
}

pub fn initialize(peripheral: LPWR<'static>) {
    critical_section::with(|cs| {
        *RTC.borrow_ref_mut(cs) = Some(Rtc::new(peripheral));
    });
}

/// This blocks until wake/rejection or the RTC deadline. A successful function
/// return supplies observations; only `Outcome::TimerWake` confirms the expected
/// hardware cycle. Peripheral recovery must complete separately.
pub fn enter() -> Result<Observation, Error> {
    critical_section::with(|cs| {
        let mut owner = RTC.borrow_ref_mut(cs);
        let rtc = owner.as_mut().ok_or(Error::Unavailable)?;
        let timer = TimerWakeupSource::new(core::time::Duration::from_secs(SLEEP_SECONDS));
        let uptime_before = Instant::now();
        // The vendored HAL keeps CPU/digital/RAM/flash/PSRAM domains and XTAL/PLL
        // powered. Neither cache suspension nor a CPU clock change is performed.
        let status =
            rtc.sleep_light_with_status(&[&timer], Duration::from_secs(ENTRY_DEADLINE_SECONDS));
        let uptime_elapsed_us = uptime_before.elapsed().as_micros();
        let outcome = if status.timed_out {
            Outcome::TimedOut
        } else if status.rejected {
            Outcome::Rejected
        } else if !status.woke || status.wakeup_cause != TIMER_WAKE_BIT {
            Outcome::UnexpectedWake
        } else if status.elapsed_us < (SLEEP_SECONDS - 1) * 1_000_000 {
            // Entry overhead is included, so even an acknowledged early return
            // cannot be reported as the requested timer sleep.
            Outcome::Immediate
        } else {
            Outcome::TimerWake
        };
        Ok(Observation {
            outcome,
            rtc_elapsed_us: status.elapsed_us,
            uptime_elapsed_us,
            wake_bits: status.wakeup_cause,
            rejected: status.rejected,
            woke: status.woke,
        })
    })
}
