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
use esp_hal::peripherals::LPWR;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::rtc_cntl::sleep::TimerWakeupSource;
use esp_hal::time::Duration;
use esp_hal::time::Instant;

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
    /// Observed uptime across the call; retention determines whether it advances.
    pub uptime_elapsed_us: u64,
    pub wake_bits: u32,
    pub rejected: bool,
    pub woke: bool,
}

/// Light sleep since the trace last took it, as the RTC measured it.
static SLEPT_US: AtomicU32 = AtomicU32::new(0);

pub fn take_slept_ms() -> u32 {
    SLEPT_US.swap(0, Ordering::Relaxed) / 1000
}

pub fn initialize(peripheral: LPWR<'static>) {
    critical_section::with(|cs| {
        *RTC.borrow_ref_mut(cs) = Some(Rtc::new(peripheral));
    });
}

/// This blocks until wake/rejection or the RTC deadline. A successful function
/// return supplies observations; only `Outcome::TimerWake` confirms the expected
/// hardware cycle. Peripheral recovery must complete separately.
/// Experiment: light sleep repeatedly for LISTEN_SECONDS, waking on companion
/// UART traffic (low level on RX GPIO41) as well as a timer.
const LISTEN_SECONDS: u64 = 30;
const GPIO_WAKE_BIT: u32 = 1 << 2;
const RX_PIN: usize = 41;
const AWAKE_SPIN_US: u64 = 4_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct Listen {
    pub sleeps: u32,
    pub gpio_wakes: u32,
    pub timer_wakes: u32,
    pub other_wakes: u32,
    pub rejected: u32,
    pub timed_out: u32,
    pub immediate_low: u32,
    pub slept_us: u64,
    pub wall_us: u64,
    pub longest_sleep_us: u64,
    pub bytes: u32,
    pub valid_frames: u32,
    pub bad_crc: u32,
    pub uart_errors: u32,
}

static LISTEN: Mutex<RefCell<Listen>> = Mutex::new(RefCell::new(Listen {
    sleeps: 0,
    gpio_wakes: 0,
    timer_wakes: 0,
    other_wakes: 0,
    rejected: 0,
    timed_out: 0,
    immediate_low: 0,
    slept_us: 0,
    wall_us: 0,
    longest_sleep_us: 0,
    bytes: 0,
    valid_frames: 0,
    bad_crc: 0,
    uart_errors: 0,
}));

pub fn listen_summary() -> Listen {
    critical_section::with(|cs| *LISTEN.borrow_ref(cs))
}

fn rx_wake(enable: bool) {
    let gpio = unsafe { &*esp32s3::GPIO::ptr() };
    gpio.pin(RX_PIN).modify(|_, w| unsafe {
        w.int_type().bits(if enable { 4 } else { 0 });
        w.wakeup_enable().bit(enable)
    });
}

fn rx_low() -> bool {
    let gpio = unsafe { &*esp32s3::GPIO::ptr() };
    gpio.in1().read().bits() & (1 << (RX_PIN - 32)) == 0
}

pub fn enter(after_wake: impl FnOnce()) -> Result<Observation, Error> {
    let mut listen = Listen::default();
    let started = Instant::now();
    let total = Duration::from_secs(LISTEN_SECONDS);
    let mut last_cause = 0;
    let mut decoder = crate::drivers::companion::Decoder::default();
    let errors_before = crate::drivers::companion_uart::drain(&mut []).1.errors();
    while started.elapsed() < total {
        let remaining = total - started.elapsed();
        let step = core::time::Duration::from_micros(remaining.as_micros().max(1_000));
        let status = critical_section::with(|cs| {
            let mut owner = RTC.borrow_ref_mut(cs);
            let rtc = owner.as_mut().ok_or(Error::Unavailable)?;
            if rx_low() {
                listen.immediate_low += 1;
            }
            let timer = TimerWakeupSource::new(step);
            let gpio = esp_hal::rtc_cntl::sleep::GpioWakeupSource::new();
            rx_wake(true);
            let status = rtc
                .sleep_light_with_status(&[&timer, &gpio], Duration::from_secs(LISTEN_SECONDS + 2));
            rx_wake(false);
            Ok::<_, Error>(status)
        })?;
        listen.sleeps += 1;
        listen.slept_us += status.elapsed_us;
        listen.longest_sleep_us = listen.longest_sleep_us.max(status.elapsed_us);
        last_cause = status.wakeup_cause;
        if status.timed_out {
            listen.timed_out += 1;
            break;
        } else if status.rejected {
            listen.rejected += 1;
        } else if status.wakeup_cause & GPIO_WAKE_BIT != 0 {
            listen.gpio_wakes += 1;
        } else if status.wakeup_cause & TIMER_WAKE_BIT != 0 {
            listen.timer_wakes += 1;
        } else {
            listen.other_wakes += 1;
        }
        // Let the UART interrupt drain the frame that woke us.
        let spin = Instant::now();
        while spin.elapsed() < Duration::from_micros(AWAKE_SPIN_US) {}
        let mut bytes = [0u8; 256];
        loop {
            let (count, _) = crate::drivers::companion_uart::drain(&mut bytes);
            if count == 0 {
                break;
            }
            listen.bytes += count as u32;
            for &byte in &bytes[..count] {
                let _ = decoder.push_frame(byte);
            }
        }
    }
    listen.wall_us = started.elapsed().as_micros();
    listen.valid_frames = decoder.valid_frames;
    listen.bad_crc = decoder.bad_crc;
    listen.uart_errors = crate::drivers::companion_uart::drain(&mut []).1.errors() - errors_before;
    SLEPT_US.fetch_add(
        listen.slept_us.min(u64::from(u32::MAX)) as u32,
        Ordering::Relaxed,
    );
    critical_section::with(|cs| *LISTEN.borrow_ref_mut(cs) = listen);
    after_wake();
    log::info!(target: "power", "sleep listen {:?}", listen);
    Ok(Observation {
        outcome: if listen.timed_out == 0 {
            Outcome::TimerWake
        } else {
            Outcome::TimedOut
        },
        rtc_elapsed_us: listen.wall_us,
        uptime_elapsed_us: listen.wall_us,
        wake_bits: last_cause,
        rejected: listen.rejected != 0,
        woke: true,
    })
}
