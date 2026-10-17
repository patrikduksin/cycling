use c606_firmware::drivers::companion_sensors::State;
use c606_firmware::drivers::companion_startup as startup;
use device_api::observation::Observation;
use device_api::sensors::Motion;
use device_api::sensors::Pressure;
use device_api::sensors::Snapshot;
use device_api::sensors::Startup;

fn sample(at: u64) -> Snapshot {
    Snapshot {
        pressure: Observation::Fresh {
            value: Pressure {
                pressure_centi_pa: 10_000_000,
                temperature_centi_c: 2000,
            },
            received_ms: at,
        },
        motion: [Observation::Fresh {
            value: Motion { axes: [0, 0, 4096] },
            received_ms: at,
        }; 2],
        ..State::default().snapshot(at, 5000)
    }
}

fn report(startup: &mut startup::Startup, reason: u8, at: u64) {
    startup.report(16, &[0xe2, 2, 0, 0, 0xff, reason, 0, 0xff], at);
}

fn prepare(startup: &mut startup::Startup, result: Result<usize, ()>) {
    startup.begin(0);
    assert!(startup.probe(State::default().snapshot(3000, 5000), 3000, true, 0, false));
    startup.submitted(result, 3100);
}

#[test]
fn startup_uses_only_the_stock_main_acknowledgment() {
    let frame = startup::frame();
    assert_eq!(
        &frame[..14],
        &[0xa5, 12, 0x6f, 0xf1, 2, 16, 0xe2, 2, 0, 0, 0, 1, 0, 0]
    );
    assert_eq!(
        u16::from_le_bytes(frame[14..].try_into().unwrap()),
        c606_firmware::drivers::companion::crc16(&frame[..14])
    );
}

#[test]
fn uart_submission_and_one_sample_do_not_establish_acquisition() {
    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    startup.observe(State::default().snapshot(3101, 8000), 3101);
    assert_eq!(startup.status(), Startup::Waiting);
    startup.observe(sample(3102), 3102);
    startup.observe(sample(3103), 3103);
    assert_eq!(startup.status(), Startup::Waiting);
    report(&mut startup, 6, 3105);
    startup.observe(sample(3090), 3102);
    startup.observe(sample(3110), 3110);
    startup.observe(sample(3110), 3120);
    assert_eq!(startup.status(), Startup::Waiting);
    let mut partial = sample(3130);
    partial.motion[1] = sample(3110).motion[1];
    startup.observe(partial, 3130);
    assert_eq!(startup.status(), Startup::Waiting);
    startup.observe(sample(3140), 3140);
    assert_eq!(startup.status(), Startup::Ready);
}

#[test]
fn failed_or_short_submission_is_terminal_and_cannot_be_replayed() {
    for result in [Err(()), Ok(0), Ok(15)] {
        let mut startup = startup::Startup::new();
        prepare(&mut startup, result);
        startup.submitted(Ok(16), 3200);
        report(&mut startup, 5, 3250);
        startup.observe(sample(3300), 3300);
        startup.observe(sample(3400), 3400);
        assert_eq!(startup.status(), Startup::Uncertain);
    }
}

#[test]
fn missing_or_stale_sensors_expire_without_a_false_success() {
    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    report(&mut startup, 5, 3100);
    startup.observe(sample(3110), 3110);
    let mut stale = sample(3200);
    let Observation::Fresh { value, received_ms } = stale.pressure else {
        unreachable!()
    };
    stale.pressure = Observation::Stale { value, received_ms };
    startup.observe(stale, 3200);
    startup.observe(sample(13100), 13100);
    assert_eq!(startup.status(), Startup::TimedOut);
    startup.submitted(Ok(16), 13101);
    startup.observe(sample(13102), 13102);
    assert_eq!(startup.status(), Startup::TimedOut);
}

#[test]
fn charging_can_wait_for_manual_on_without_a_sensor_deadline() {
    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    report(&mut startup, 4, 3200);
    assert_eq!(startup.reason(), Some(4));
    startup.observe(sample(63000), 63000);
    assert_eq!(startup.status(), Startup::Charging);
    report(&mut startup, 4, 63001);
    report(&mut startup, 5, 63100);
    startup.observe(sample(63100), 63100);
    startup.observe(sample(63110), 63110);
    assert_eq!(startup.status(), Startup::Waiting);
    startup.observe(sample(63120), 63120);
    assert_eq!(startup.status(), Startup::Ready);
    assert_eq!(startup.reason(), Some(5));
}

#[test]
fn malformed_and_unknown_reports_cannot_start_or_extend_a_wait() {
    let valid = [0xe2, 2, 0, 0, 0xff, 5, 0, 0xff];
    let mut startup = startup::Startup::new();
    startup.report(16, &valid, 0);
    assert_eq!(startup.reason(), None);
    prepare(&mut startup, Ok(16));
    startup.report(1, &valid, 3200);
    startup.report(16, &valid[..7], 3200);
    startup.report(16, &[0xe2, 2, 0, 0, 0xff, 5, 0, 0xff, 0], 3200);
    for index in 0..8 {
        let mut invalid = valid;
        invalid[index] ^= 0x80;
        startup.report(16, &invalid, 3200);
    }
    for reason in [0, 1, 2, 3, 7, 255] {
        report(&mut startup, reason, 3200);
    }
    assert_eq!(startup.reason(), None);
    report(&mut startup, 4, 13100);
    assert_eq!(startup.status(), Startup::TimedOut);
    report(&mut startup, 5, 13101);
    assert_eq!(startup.reason(), None);
}

#[test]
fn repeated_reasons_preserve_deadline_and_first_sample() {
    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    report(&mut startup, 6, 3200);
    startup.observe(sample(3300), 3300);
    report(&mut startup, 6, 3400);
    report(&mut startup, 4, 3401);
    report(&mut startup, 5, 3402);
    startup.observe(sample(3500), 3500);
    assert_eq!(startup.status(), Startup::Ready);
    assert_eq!(startup.reason(), Some(6));

    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    report(&mut startup, 5, 3200);
    report(&mut startup, 5, 13199);
    startup.observe(sample(13200), 13200);
    assert_eq!(startup.status(), Startup::TimedOut);
}

#[test]
fn each_sensor_must_advance_after_operating_reason() {
    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    report(&mut startup, 5, 3200);
    let mut old = sample(3210);
    old.motion[1] = sample(3200).motion[1];
    startup.observe(old, 3210);
    let mut first = sample(3220);
    first.pressure = sample(3210).pressure;
    first.motion[1] = sample(3215).motion[1];
    startup.observe(first, 3220);
    let mut partial = sample(3230);
    partial.motion[0] = sample(3220).motion[0];
    startup.observe(partial, 3230);
    assert_eq!(startup.status(), Startup::Waiting);
    let mut unavailable = sample(3240);
    unavailable.motion[1] = Observation::Unavailable;
    startup.observe(unavailable, 3240);
    startup.observe(sample(4000), 3250);
    assert_eq!(startup.status(), Startup::Waiting);
    let mut next = sample(3260);
    next.pressure = sample(3211).pressure;
    next.motion[1] = sample(3216).motion[1];
    startup.observe(next, 3260);
    assert_eq!(startup.status(), Startup::Ready);
}

#[test]
fn passive_healthy_startup_preserves_running_companion() {
    let mut startup = startup::Startup::new();
    startup.begin(100);
    assert!(!startup.ant_allowed());
    assert!(!startup.probe(sample(200), 200, true, 0, false));
    startup.begin(2000);
    assert!(!startup.probe(sample(3100), 3100, true, 0, false));
    assert_eq!(startup.status(), Startup::AlreadyRunning);
    assert!(startup.ant_allowed());
    startup.submitted(Ok(16), 3200);
    assert_eq!(startup.status(), Startup::AlreadyRunning);
}

#[test]
fn partial_invalid_sensor_or_radio_traffic_suppresses_ack() {
    for radio in [false, true] {
        let mut startup = startup::Startup::new();
        startup.begin(100);
        if !radio {
            // Known sensor subtype, but malformed length and no decoded sample.
            startup.report(16, &[0xf1, 2], 200);
        }
        let empty = State::default().snapshot(200, 5000);
        assert!(!startup.probe(empty, 200, true, 0, radio));
        assert!(!startup.probe(empty, 3100, true, 0, false));
        assert_eq!(startup.status(), Startup::RunningDegraded);
        assert!(startup.ant_allowed());
    }
    let mut startup = startup::Startup::new();
    startup.begin(100);
    let mut partial = sample(200);
    partial.motion[1] = Observation::Unavailable;
    assert!(!startup.probe(partial, 200, true, 0, false));
    assert!(!startup.probe(partial, 3100, true, 0, false));
    assert_eq!(startup.status(), Startup::RunningDegraded);
}

#[test]
fn cold_silent_companion_gets_one_ack_without_waiting_for_battery_reports() {
    let mut startup = startup::Startup::new();
    let empty = State::default().snapshot(0, 5000);
    startup.begin(0);
    assert!(!startup.probe(empty, 2999, false, 0, false));
    // A cold companion can withhold battery/power reports until this acknowledgment.
    assert!(startup.probe(empty, 3000, false, 0, false));
    assert_eq!(startup.status(), Startup::Pending);
    assert!(!startup.probe(empty, 3001, false, 0, false));
    startup.submitted(Ok(16), 3001);
    assert!(!startup.probe(empty, 6000, false, 0, false));
    assert!(!startup.probe(empty, 6001, true, 0, false));
    assert_eq!(startup.status(), Startup::Waiting);
}

#[test]
fn transient_transport_loss_forbids_ack_until_a_clean_window() {
    let mut startup = startup::Startup::new();
    let empty = State::default().snapshot(0, 5000);
    startup.begin(0);
    // The fault count is cumulative: one fault at 100 ms, none after it.
    assert!(!startup.probe(empty, 0, false, 0, false));
    assert!(!startup.probe(empty, 100, false, 1, false));
    // The window that saw it is observed again, never acknowledged.
    assert!(!startup.probe(empty, 3000, true, 1, false));
    assert_eq!(startup.status(), Startup::Probing);
    assert!(!startup.probe(empty, 8999, true, 1, false));
    // The second, six-second window is clean: a silent companion gets its ACK.
    assert!(startup.probe(empty, 9000, true, 1, false));
    assert_eq!(startup.status(), Startup::Pending);
}

/// Probe every 100 ms; `faults(now)` is the cumulative fault count.
fn run(
    startup: &mut startup::Startup,
    span: core::ops::Range<u64>,
    snapshot: impl Fn(u64) -> Snapshot,
    faults: impl Fn(u64) -> u32,
) -> bool {
    let mut acknowledge = false;
    for now in span.step_by(100) {
        acknowledge |= startup.probe(snapshot(now), now, true, faults(now), false);
    }
    acknowledge
}

#[test]
fn faults_counted_before_the_check_do_not_taint_it() {
    let mut startup = startup::Startup::new();
    startup.begin(0);
    assert!(!run(&mut startup, 0..3200, sample, |_| 1));
    assert_eq!(startup.status(), Startup::AlreadyRunning);
}

#[test]
fn one_fault_while_a_running_companion_is_checked_is_not_fatal() {
    // Observed on hardware: a software restart, one UART fault during the check.
    let mut startup = startup::Startup::new();
    startup.begin(0);
    let faults = |now| u32::from(now >= 500);
    assert!(!run(&mut startup, 0..3100, sample, faults));
    assert_eq!(startup.status(), Startup::Probing);
    assert!(!run(&mut startup, 3100..9300, sample, faults));
    assert_eq!(startup.status(), Startup::AlreadyRunning);
}

#[test]
fn a_fault_on_the_retry_tick_belongs_to_the_window_it_ends() {
    // The first window ends at 3000 ms; a fault counted on that very tick
    // retries, and the next window, clean from then on, finds the companion.
    let mut startup = startup::Startup::new();
    startup.begin(0);
    let faults = |now| u32::from(now >= 3000);
    assert!(!run(&mut startup, 0..3100, sample, faults));
    assert_eq!(startup.status(), Startup::Probing);
    assert!(!run(&mut startup, 3100..9100, sample, faults));
    assert_eq!(startup.status(), Startup::AlreadyRunning);
}

#[test]
fn faults_on_every_retry_tick_still_end_with_a_clean_window() {
    // A fault on each window's last tick, then none: four retries would give
    // up, so the counts must not leak into the windows they end.
    let mut startup = startup::Startup::new();
    startup.begin(0);
    // Windows of 3, 6 and 12 seconds end at 3, 9 and 21 s; the fourth is clean.
    let ends = [3000, 9000, 21_000];
    let faults = |now: u64| ends.iter().filter(|end| now >= **end).count() as u32;
    assert!(!run(&mut startup, 0..21_100, sample, faults));
    assert_eq!(startup.status(), Startup::Probing);
    assert!(!run(&mut startup, 21_100..45_100, sample, faults));
    assert_eq!(startup.status(), Startup::AlreadyRunning);
}

#[test]
fn persistent_transport_faults_give_up_after_bounded_windows() {
    let mut startup = startup::Startup::new();
    let empty = State::default().snapshot(0, 5000);
    startup.begin(0);
    // A new fault every tick: windows of 3, 6, 12 and 24 seconds.
    let faults = |now: u64| (now / 100) as u32;
    assert!(!run(&mut startup, 0..44_900, |_| empty, faults));
    assert_eq!(startup.status(), Startup::Probing);
    assert!(!run(&mut startup, 44_900..45_300, |_| empty, faults));
    assert_eq!(startup.status(), Startup::Unavailable);
    // A companion it could not observe cleanly is never acknowledged.
    assert!(!run(&mut startup, 45_300..60_000, |_| empty, faults));
}

#[test]
fn silent_sensors_allow_exactly_one_guarded_recovery_dispatch() {
    let mut startup = startup::Startup::new();
    let empty = State::default().snapshot(0, 5000);
    startup.submitted(Ok(16), 0);
    assert_eq!(startup.status(), Startup::NotSubmitted);
    startup.begin(100);
    startup.report(16, &[0xf1, 4], 200);
    startup.report(1, &[0xf1, 1], 200);
    assert!(!startup.probe(empty, 3099, true, 0, false));
    assert!(startup.probe(empty, 3100, true, 0, false));
    assert_eq!(startup.status(), Startup::Pending);
    assert!(!startup.ant_allowed());
    assert!(!startup.probe(empty, 3200, true, 0, false));
    startup.submitted(Ok(16), 3200);
    report(&mut startup, 4, 3300);
    assert!(!startup.ant_allowed());
    report(&mut startup, 5, 60_000);
    startup.observe(sample(60_001), 60_001);
    startup.observe(sample(60_002), 60_002);
    assert_eq!(startup.status(), Startup::Ready);
    assert!(startup.ant_allowed());
}

fn charging_startup() -> startup::Startup {
    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    report(&mut startup, 4, 3200);
    assert_eq!(startup.status(), Startup::Charging);
    startup
}

#[test]
fn wake_frame_is_fixed_operation_zero_value_seven_with_valid_crc() {
    let frame = startup::wake_frame();
    assert_eq!(
        &frame[..14],
        &[0xa5, 12, 0x6f, 0xf1, 2, 16, 0xe2, 2, 0, 0, 0, 7, 0, 0]
    );
    assert_eq!(
        u16::from_le_bytes(frame[14..].try_into().unwrap()),
        c606_firmware::drivers::companion::crc16(&frame[..14])
    );
}

#[test]
fn charging_allows_one_wake_then_requires_new_operating_reason_and_sample_progress() {
    for reason in [5, 6] {
        let empty = State::default().snapshot(0, 5000);
        let mut startup = charging_startup();
        startup.request_wake().unwrap();
        assert!(!startup.wake(empty, false, 0, false));
        assert_eq!(startup.status(), Startup::Charging);
        assert!(startup.wake(empty, true, 0, false));
        assert_eq!(startup.status(), Startup::WakePending);
        assert!(!startup.ant_allowed());
        assert!(!startup.wake(empty, true, 0, false));
        startup.wake_submitted(Ok(16), 3500);
        startup.observe(sample(3510), 3510);
        startup.observe(sample(3520), 3520);
        assert_eq!(
            startup.status(),
            Startup::Waiting,
            "sensor progress alone is not an operating reason"
        );
        report(&mut startup, reason, 3600);
        startup.observe(sample(3600), 3600);
        startup.observe(sample(3610), 3610);
        let mut partial = sample(3620);
        partial.motion[1] = sample(3610).motion[1];
        startup.observe(partial, 3620);
        assert_eq!(startup.status(), Startup::Waiting);
        startup.observe(sample(3630), 3630);
        assert_eq!(startup.status(), Startup::Ready);
        assert!(startup.ant_allowed());
        assert!(!startup.wake(empty, true, 0, false));
    }
}

#[test]
fn wake_missing_reason_or_partial_submission_never_retries_or_claims_ready() {
    let empty = State::default().snapshot(0, 5000);
    for result in [Err(()), Ok(0), Ok(15), Ok(16)] {
        let mut startup = charging_startup();
        startup.request_wake().unwrap();
        assert!(startup.wake(empty, true, 0, false));
        startup.wake_submitted(result, 3500);
        // Repeated completion calls cannot restart or repair a failed submission.
        startup.wake_submitted(Ok(16), 4000);
        report(&mut startup, 4, 5000);
        startup.observe(sample(5010), 5010);
        startup.observe(sample(5020), 5020);
        startup.observe(sample(13500), 13500);
        assert_eq!(
            startup.status(),
            if result == Ok(16) {
                Startup::TimedOut
            } else {
                Startup::Uncertain
            }
        );
        assert!(!startup.ant_allowed());
        assert!(!startup.wake(empty, true, 0, false));
        report(&mut startup, 5, 14000);
        startup.observe(sample(14010), 14010);
        startup.observe(sample(14020), 14020);
        assert_ne!(startup.status(), Startup::Ready);
    }
}

#[test]
fn existing_sensor_or_radio_activity_and_transport_loss_suppress_wake() {
    let empty = State::default().snapshot(0, 5000);
    for source in 0..5 {
        let mut startup = charging_startup();
        startup.request_wake().unwrap();
        let mut observation = empty;
        match source {
            0 => observation.pressure = sample(3210).pressure,
            1 => observation.motion[0] = sample(3210).motion[0],
            2 => startup.report(16, &[0xf1, 3], 3210),
            _ => {}
        }
        let faults = u32::from(source == 4);
        assert!(!startup.wake(observation, true, faults, source == 3));
        assert_eq!(
            startup.status(),
            if source == 4 {
                // A fault postpones the decision; it is not final.
                Startup::Charging
            } else {
                Startup::RunningDegraded
            }
        );
        // A fault-free tick lets the authorized wake proceed exactly once.
        assert_eq!(startup.wake(empty, true, faults, false), source == 4);
        assert!(!startup.wake(empty, true, faults, false));
    }
}

#[test]
fn sensor_activity_while_waiting_for_ack_suppresses_later_wake() {
    let mut startup = startup::Startup::new();
    prepare(&mut startup, Ok(16));
    startup.report(16, &[0xf1, 2], 3150);
    report(&mut startup, 4, 3200);
    startup.request_wake().unwrap();
    assert!(!startup.wake(State::default().snapshot(3201, 5000), true, 0, false));
    assert_eq!(startup.status(), Startup::RunningDegraded);
}

#[test]
fn charging_standby_does_not_automatically_return_to_normal_operation() {
    let mut startup = charging_startup();
    let empty = State::default().snapshot(3201, 5000);
    assert!(!startup.wake(empty, true, 0, false));
    assert_eq!(startup.status(), Startup::Charging);
}
