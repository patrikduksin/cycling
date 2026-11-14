//! Bounded ESP32-S3 light sleep with RTC evidence and retained volatile memory.
//!
//! Call only after device work and DMA have quiesced. The C606 starts one CPU;
//! the critical section below prevents that CPU's scheduler and interrupt tasks
//! from touching peripherals during entry. This is not a second-core parking API.
//! Callers must invalidate acquisition stamps across the receive gap. Compare
//! RTC and uptime intervals rather than assuming which retained clocks advance.
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};
use critical_section::Mutex;
use device_api::observation::Error;
use esp_hal::peripherals::{LPWR, RTC_TIMER};
use esp_hal::rtc_cntl::sleep::{LightSleepOutcome, LowPower, RtcSleepConfig};
use esp_hal::rtc_cntl::{Rtc, WakeupReason, WakeupSource, wakeup_cause};
use esp_hal::time::Duration;
use esp_hal::time::Instant;

const SLEEP_SECONDS: u64 = 10;
const ENTRY_DEADLINE_SECONDS: u64 = 12;

struct Sleeper {
    low_power: LowPower<'static>,
    rtc: Rtc<'static>,
}

static SLEEPER: Mutex<RefCell<Option<Sleeper>>> = Mutex::new(RefCell::new(None));

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
    /// RTC time across the call, including entry overhead.
    pub rtc_elapsed_us: u64,
    /// Observed uptime across the call; retention determines whether it advances.
    pub uptime_elapsed_us: u64,
    pub hardware: LightSleepOutcome,
    pub cause: WakeupReason,
}

/// Light sleep since the trace last took it, as the RTC measured it.
static SLEPT_US: AtomicU32 = AtomicU32::new(0);

pub fn take_slept_ms() -> u32 {
    SLEPT_US.swap(0, Ordering::Relaxed) / 1000
}

pub fn initialize(lpwr: LPWR<'static>, rtc_timer: RTC_TIMER<'static>) {
    critical_section::with(|cs| {
        *SLEEPER.borrow_ref_mut(cs) = Some(Sleeper {
            low_power: LowPower::new(lpwr),
            rtc: Rtc::new(rtc_timer),
        });
    });
}

/// Light sleep that keeps the CPU, digital peripherals, RTC memory and
/// peripherals, main RAM, the flash/PSRAM supply, the modem domain, the XTAL
/// and the PLL powered. Neither cache suspension nor a CPU clock change is
/// performed. The XTAL stays up, so the regulator and bias keep their XTAL-up
/// settings rather than the XTAL-down ones of the HAL's light-sleep default.
fn retained_light_sleep() -> RtcSleepConfig {
    let mut config = RtcSleepConfig::default();
    config.set_rtc_regulator_fpu(false);
    config.set_bias_sleep_monitor(false);
    config.set_pd_cur_monitor(false);
    config.set_bias_sleep_slp(false);
    config.set_pd_cur_slp(false);
    config.set_xtal_fpu(true);
    config.set_lslp_mem_inf_fpu(true);
    config.set_cpu_pd_en(false);
    config.set_dig_peri_pd_en(false);
    config.set_rtc_fastmem_pd_en(false);
    config.set_rtc_slowmem_pd_en(false);
    config.set_rtc_peri_pd_en(false);
    config.set_vddsdio_pd_en(false);
    config.set_modem_pd_en(false);
    config
}

/// This blocks until wake/rejection or the RTC deadline. A successful function
/// return supplies observations; only `Outcome::TimerWake` confirms the expected
/// hardware cycle. Peripheral recovery must complete separately.
pub fn enter(after_wake: impl FnOnce()) -> Result<Observation, Error> {
    critical_section::with(|cs| {
        let mut owner = SLEEPER.borrow_ref_mut(cs);
        let Sleeper { low_power, rtc } = owner.as_mut().ok_or(Error::Unavailable)?;
        let uptime_before = Instant::now();
        let rtc_before = rtc.time_since_power_up();
        // The timer is the only armed wake source, and only for this call.
        low_power.set_wakeup_deadline(uptime_before + Duration::from_secs(SLEEP_SECONDS));
        let hardware = low_power.sleep_light_with_status(
            retained_light_sleep(),
            Duration::from_secs(ENTRY_DEADLINE_SECONDS),
        );
        let rtc_elapsed_us = (rtc.time_since_power_up() - rtc_before).as_micros();
        low_power.clear_wakeup_deadline();
        let uptime_elapsed_us = uptime_before.elapsed().as_micros();
        let cause = wakeup_cause();
        SLEPT_US.fetch_add(
            rtc_elapsed_us.min(u64::from(u32::MAX)) as u32,
            Ordering::Relaxed,
        );
        after_wake();
        let only_timer = cause.contains(WakeupSource::Timer) && cause.iter().count() == 1;
        let outcome = match hardware {
            LightSleepOutcome::TimedOut => Outcome::TimedOut,
            LightSleepOutcome::Rejected => Outcome::Rejected,
            LightSleepOutcome::NoWakeupSource => Outcome::UnexpectedWake,
            LightSleepOutcome::Woke if !only_timer => Outcome::UnexpectedWake,
            // Entry overhead is included, so even an acknowledged early return
            // cannot be reported as the requested timer sleep.
            LightSleepOutcome::Woke if rtc_elapsed_us < (SLEEP_SECONDS - 1) * 1_000_000 => {
                Outcome::Immediate
            }
            LightSleepOutcome::Woke => Outcome::TimerWake,
        };
        Ok(Observation {
            outcome,
            rtc_elapsed_us,
            uptime_elapsed_us,
            hardware,
            cause,
        })
    })
}
