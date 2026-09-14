//! One C606 owner coordinates hardware shutdown independently of the shell.
use crate::device::power_transition::{Gate, Transition};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use cycling_os::{
    capabilities::Error,
    power::{
        Capabilities, Failure, Operation, PeripheralState, State, Status, Support, WakeSources,
    },
};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Instant, Timer};
use esp_hal::ledc::{
    LowSpeed,
    channel::{Channel, ChannelIFace},
};

pub static ACCESS: Gate = Gate::new();
static TRANSITION: Mutex<CriticalSectionRawMutex, RefCell<Transition>> =
    Mutex::new(RefCell::new(Transition::new()));
struct Backlight {
    channel: Channel<'static, LowSpeed>,
    percent: u8,
}
static BRIGHTNESS: AtomicU8 = AtomicU8::new(0);
static LIGHT_FAILED: AtomicBool = AtomicBool::new(false);

/// Accept a brightness target. The sole LEDC owner applies it on its next tick;
/// driver failure is observable through the Power capability's availability.
pub fn brightness(percent: u8) -> Result<(), Error> {
    let _access = ACCESS.enter()?;
    if percent > 100 {
        return Err(Error::Invalid);
    }
    if LIGHT_FAILED.load(Ordering::Acquire) {
        return Err(Error::Failed);
    }
    BRIGHTNESS.store(percent, Ordering::Release);
    Ok(())
}
pub fn light_available() -> bool {
    !LIGHT_FAILED.load(Ordering::Acquire)
}
fn backlight(light: &mut Backlight, suspend: bool) -> Result<(), Error> {
    let percent = if suspend {
        0
    } else {
        BRIGHTNESS.load(Ordering::Acquire)
    };
    let result = light.channel.set_duty(percent).map_err(|_| Error::Failed);
    LIGHT_FAILED.store(result.is_err(), Ordering::Release);
    if result.is_ok() {
        light.percent = percent;
    }
    result
}
pub fn capabilities() -> Capabilities {
    // Stock establishes the fixed shutdown operation. Electrical effects and
    // selectable wake sources remain unknown until physical validation.
    let unknown = WakeSources {
        button: Support::Unknown,
        usb: Support::Unknown,
    };
    Capabilities {
        shutdown: Support::Supported,
        sleep: Support::Unknown,
        wake: Support::Supported,
        shutdown_wake: WakeSources {
            button: Support::Supported,
            usb: Support::Unknown,
        },
        sleep_wake: unknown,
    }
}
pub fn status() -> Status {
    let mut status = TRANSITION.lock(|t| t.borrow().status());
    if status.state == State::Idle
        && !matches!(
            super::sensors::startup_phase(),
            "ready" | "already_running" | "running_degraded"
        )
    {
        status.state = State::Initializing;
        status.ready = false;
    }
    status
}
pub fn request(operation: Operation, now: u64) -> Result<(), Error> {
    request_after(operation, 0, now)
}
pub fn request_after(operation: Operation, delay_ms: u32, now: u64) -> Result<(), Error> {
    if delay_ms > cycling_os::power::MAX_DELAY_MS
        || (delay_ms != 0 && operation != Operation::Shutdown)
    {
        return Err(Error::Invalid);
    }
    if operation == Operation::Wake {
        if status().state != State::Charging {
            return Err(Error::Unavailable);
        }
        super::sensors::power_request_wake()?;
        return TRANSITION.lock(|t| t.borrow_mut().wake(now));
    }
    if !matches!(
        super::sensors::startup_status(),
        "ready" | "already_running"
    ) {
        return Err(Error::Unavailable);
    }
    TRANSITION.lock(|t| {
        if delay_ms == 0 {
            t.borrow_mut().request(operation, now)?;
        } else {
            t.borrow_mut().request_after(operation, delay_ms, now)?;
        }
        ACCESS.close();
        Ok(())
    })
}
fn abort(failure: Failure, now: u64) {
    TRANSITION.lock(|t| t.borrow_mut().abort(failure, now));
}

#[derive(Default)]
struct Work {
    charging: bool,
    wifi: bool,
    ble: bool,
    ant: bool,
    gnss: bool,
    sound: bool,
    dark: bool,
    restoring: bool,
    restore_failed: bool,
    uart: Option<(u32, u32)>,
    sleep_boundary: Option<u64>,
}
fn begin(started: &mut bool, request: impl FnOnce() -> Result<(), Error>) -> Result<(), Error> {
    if !*started {
        match request() {
            Ok(()) => *started = true,
            // The owner has not accepted this request; it is safe to wait and
            // try admission again. Accepted operations are never resubmitted.
            Err(Error::Unavailable) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[embassy_executor::task]
pub async fn run(channel: Channel<'static, LowSpeed>) {
    let mut light = Backlight {
        channel,
        percent: 0,
    };
    let mut work = Work::default();
    let mut previous = State::Idle;
    loop {
        let now = Instant::now().as_millis();
        let phase = super::sensors::startup_phase();
        if TRANSITION.lock(|t| t.borrow().status().state) == State::Idle && phase == "charging" {
            ACCESS.close();
            super::io::power_boundary(now);
            work = Work {
                charging: true,
                uart: super::io::snapshot(now).map(|s| (s.uart_errors, s.companion_bad_crc)),
                ..Work::default()
            };
            TRANSITION.lock(|t| t.borrow_mut().begin_charging(now));
        }
        if work.charging
            && matches!(status().state, State::Charging | State::Quiescing)
            && matches!(super::sensors::startup_reason(), Some(5 | 6))
        {
            // The companion itself owns this physical button transition. Do not
            // send a second initialization command in response to its report.
            TRANSITION.lock(|t| t.borrow_mut().physical_wake(now));
        }
        if status().ready && !ACCESS.closed() && light.percent != BRIGHTNESS.load(Ordering::Acquire)
        {
            let _ = backlight(&mut light, false);
        }
        TRANSITION.lock(|t| t.borrow_mut().tick(now));
        match status().state {
            State::Requested => {
                work = Work::default();
                if status().prepare_at_ms.is_some_and(|at| now >= at) && ACCESS.drained() {
                    work.uart =
                        super::io::snapshot(now).map(|s| (s.uart_errors, s.companion_bad_crc));
                    super::io::power_boundary(now);
                    TRANSITION.lock(|t| t.borrow_mut().quiescing(now));
                }
            }
            State::Quiescing => prepare(&mut work, &mut light, now),
            State::Recovering => recover(&mut work, &mut light, now),
            _ => {}
        }
        let current = status();
        if current.state != previous {
            log::info!(target: "power", "sequence={} state={:?} failure={:?} ready={}", current.sequence, current.state, current.failure, current.ready);
            previous = current.state;
        }
        Timer::after_millis(10).await;
    }
}

fn prepare(work: &mut Work, light: &mut Backlight, now: u64) {
    let errors = super::io::snapshot(now).map(|s| (s.uart_errors, s.companion_bad_crc));
    if work.uart.is_none() || errors != work.uart {
        abort(Failure::Companion, now);
        return;
    }
    let requests = [
        (
            begin(&mut work.wifi, || crate::device::wifi::power_request(true)),
            Failure::Wifi,
        ),
        (
            begin(&mut work.ble, || {
                crate::device::bluetooth::power_request(true)
            }),
            Failure::Ble,
        ),
        (
            begin(&mut work.ant, || super::ant::power_request(true, now)),
            Failure::Ant,
        ),
        (
            begin(&mut work.gnss, || {
                super::position_control::power_suspend(now)
            }),
            Failure::Gnss,
        ),
        (
            begin(&mut work.sound, || super::sound::power_stop(now)),
            Failure::Sound,
        ),
    ];
    for (result, failure) in requests {
        if result.is_err() {
            abort(failure, now);
            return;
        }
    }
    let radios = [
        (
            work.wifi,
            crate::device::wifi::power_status(),
            Failure::Wifi,
        ),
        (
            work.ble,
            crate::device::bluetooth::power_status(),
            Failure::Ble,
        ),
        (work.ant, super::ant::power_status(), Failure::Ant),
    ];
    for &(started, state, failure) in &radios {
        if started && state == PeripheralState::Failed {
            abort(failure, now);
            return;
        }
    }
    let gnss = super::position_control::snapshot();
    let sound = super::sound::snapshot();
    if work.gnss && gnss.state == cycling_os::position_control::State::Failed {
        abort(Failure::Gnss, now);
        return;
    }
    if work.sound && sound.state == cycling_os::sound::State::Failed {
        abort(Failure::Sound, now);
        return;
    }
    if !radios
        .iter()
        .all(|(started, state, _)| *started && *state == PeripheralState::Quiescent)
        || !work.gnss
        || gnss.state != cycling_os::position_control::State::SilenceObserved
        || !work.sound
        || sound.state != cycling_os::sound::State::Elapsed
        || !ACCESS.drained()
    {
        return;
    }
    // Display DMA and flash are synchronous and hold access leases until return.
    // Admission remains closed through command submission and any uncertain result.
    work.dark = true;
    if backlight(light, true).is_err() {
        abort(Failure::Display, now);
        return;
    }
    if work.charging {
        TRANSITION.lock(|t| t.borrow_mut().charging(now));
        return;
    }
    if status().operation == Some(Operation::Sleep) {
        super::io::sleep_boundary(now);
        TRANSITION.lock(|t| t.borrow_mut().sleeping(now));
        log::info!(target: "power", "sleep entering timer_seconds=10 companion=running");
        let observed = crate::device::sleep::enter(|| {
            // Discard the intentional UART gap before pending interrupts run.
            super::io::sleep_boundary(Instant::now().as_millis());
        });
        if let Ok(observed) = observed {
            cycling_os::network_time::sleep_elapsed(
                observed.rtc_elapsed_us,
                observed.uptime_elapsed_us,
            );
        }
        log::info!(target: "power", "sleep returned {:?}", observed);
        let now = Instant::now().as_millis();
        work.sleep_boundary = Some(now);
        let slept = observed.is_ok_and(|o| o.outcome == crate::device::sleep::Outcome::TimerWake);
        let uncertain = observed.is_ok_and(|o| {
            matches!(
                o.outcome,
                crate::device::sleep::Outcome::TimedOut
                    | crate::device::sleep::Outcome::UnexpectedWake
            )
        });
        TRANSITION.lock(|t| {
            if uncertain {
                t.borrow_mut().sleep_uncertain(now);
            } else {
                t.borrow_mut().sleep_returned(slept, now);
            }
        });
        return;
    }
    super::sensors::loss();
    super::ant::loss(now);
    super::positioning::invalidate();
    let result =
        crate::device::companion_uart::send(&crate::device::power_transition::shutdown_frame());
    TRANSITION.lock(|t| t.borrow_mut().submitted(result, now));
}

fn recover(work: &mut Work, light: &mut Backlight, now: u64) {
    // Failed charging preparation must not silently turn radios/GNSS back on.
    // Only an operating-reason report or accepted Wake requests active recovery.
    if work.charging && status().operation != Some(Operation::Wake) {
        TRANSITION.lock(|t| t.borrow_mut().standby_failed());
        return;
    }
    if !work.restoring {
        work.restoring = true;
        work.restore_failed |= work.dark && backlight(light, false).is_err();
        work.restore_failed |= work.wifi && crate::device::wifi::power_request(false).is_err();
        work.restore_failed |= work.ble && crate::device::bluetooth::power_request(false).is_err();
        work.restore_failed |= work.ant && super::ant::power_request(false, now).is_err();
        work.restore_failed |= work.gnss && super::position_control::power_resume(now).is_err();
    }
    let recovery = crate::device::power_transition::recovery_status(
        [
            work.wifi.then(crate::device::wifi::power_status),
            work.ble.then(crate::device::bluetooth::power_status),
            work.ant.then(super::ant::power_status),
        ],
        work.gnss.then(|| super::position_control::snapshot().state),
        work.sound.then(|| super::sound::snapshot().state),
    );
    if work.restore_failed || recovery == PeripheralState::Failed {
        TRANSITION.lock(|t| t.borrow_mut().recovery_failed());
    } else if recovery == PeripheralState::Running
        && work.sleep_boundary.is_none_or(|at| {
            crate::device::power_transition::sensors_after(super::sensors::snapshot(now), at)
        })
        && (!work.charging
            || matches!(super::sensors::startup_phase(), "ready" | "already_running"))
    {
        TRANSITION.lock(|t| t.borrow_mut().recovered());
        super::io::power_boundary(now);
        ACCESS.open();
    }
}
