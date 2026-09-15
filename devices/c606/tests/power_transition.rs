use c606_firmware::drivers::power_transition as transition;
use device_api::observation::Error;
use device_api::power::Failure;
use device_api::power::Operation;
use device_api::power::State;
use transition::Gate;
use transition::Transition;

#[test]
fn delayed_shutdown_waits_without_spending_the_preparation_budget() {
    let mut t = Transition::new();
    t.request_after(Operation::Shutdown, 30_000, 100).unwrap();
    assert_eq!(t.status().prepare_at_ms, Some(30_100));
    assert!(!t.status().ready);
    let accepted = t.status();
    assert_eq!(
        t.request_after(Operation::Shutdown, 1, 200),
        Err(Error::Unavailable)
    );
    assert_eq!(t.status(), accepted);
    t.tick(30_099);
    t.quiescing(30_099);
    assert_eq!(t.status().state, State::Requested);
    t.submitted(Ok(()), 30_099);
    assert_eq!(t.status().state, State::Requested);
    t.tick(30_100);
    t.quiescing(30_100);
    assert_eq!(t.status().state, State::Quiescing);
    t.tick(60_099);
    assert_eq!(t.status().state, State::Quiescing);
    t.submitted(Ok(()), 60_099);
    let submitted = t.status();
    t.submitted(Ok(()), 60_100);
    assert_eq!(t.status(), submitted);
    t.tick(60_099 + transition::OBSERVE_MS);
    assert_eq!(t.status().state, State::Uncertain);
    assert!(!t.status().ready);
}

#[test]
fn delayed_request_still_bounds_waiting_for_accepted_io() {
    let mut t = Transition::new();
    t.request_after(Operation::Shutdown, 30_000, 100).unwrap();
    t.tick(60_099);
    assert_eq!(t.status().state, State::Requested);
    t.tick(60_100);
    assert_eq!(t.status().state, State::Recovering);
    assert_eq!(t.status().failure, Some(Failure::PreparationTimeout));
}

#[test]
fn delayed_shutdown_validates_before_mutating_the_transition() {
    let mut t = Transition::new();
    for (operation, delay) in [
        (Operation::Shutdown, 30_001),
        (Operation::Sleep, 1),
        (Operation::Wake, 1),
    ] {
        assert_eq!(t.request_after(operation, delay, 100), Err(Error::Invalid));
        assert_eq!(t.status(), device_api::power::Status::IDLE);
    }
    t.request_after(Operation::Shutdown, 0, 100).unwrap();
    t.quiescing(100);
    assert_eq!(t.status().state, State::Quiescing);
}

#[test]
fn terminal_shutdown_delay_is_bounded_and_has_no_extra_arguments() {
    use firmware_console::protocol::Command;
    use firmware_console::protocol::parse;
    for delay in [0, 1, 30_000] {
        assert_eq!(
            parse(format!("CMD 1 POWER SHUTDOWN AFTER {delay}").as_bytes())
                .unwrap()
                .command,
            Command::PowerShutdownAfter(delay)
        );
    }
    for command in [
        "CMD 1 POWER SHUTDOWN AFTER",
        "CMD 1 POWER SHUTDOWN AFTER -1",
        "CMD 1 POWER SHUTDOWN AFTER 30001",
        "CMD 1 POWER SHUTDOWN AFTER 4294967296",
        "CMD 1 POWER SHUTDOWN AFTER 1000 extra",
        "CMD 1 POWER SHUTDOWN 1000",
        "CMD 1 POWER SLEEP AFTER 1000",
        "CMD 1 POWER WAKE AFTER 1000",
    ] {
        assert!(parse(command.as_bytes()).is_err(), "{command}");
    }
}

#[test]
fn accepted_io_must_finish_and_new_io_is_rejected_through_uncertainty() {
    let gate = Gate::new();
    let write = gate.enter().unwrap();
    gate.close();
    assert!(gate.closed());
    assert!(!gate.drained());
    assert!(matches!(gate.enter(), Err(Error::Unavailable)));
    drop(write);
    assert!(gate.drained());
    assert!(matches!(gate.enter(), Err(Error::Unavailable)));
    gate.open();
    assert!(gate.enter().is_ok());
}

#[test]
fn gate_closure_cannot_miss_a_racing_entrant() {
    use std::sync::Arc;
    use std::sync::Barrier;
    for _ in 0..32 {
        let gate = Arc::new(Gate::new());
        let barrier = Arc::new(Barrier::new(2));
        let other_gate = gate.clone();
        let other_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            let lease = other_gate.enter();
            other_barrier.wait();
            if lease.is_ok() {
                assert!(!other_gate.drained());
            }
            other_barrier.wait();
        });
        gate.close();
        barrier.wait();
        assert!(gate.enter().is_err());
        barrier.wait();
        worker.join().unwrap();
        assert!(gate.drained());
    }
}

#[test]
fn duplicate_request_does_not_change_shutdown() {
    let mut t = Transition::new();
    assert_eq!(t.status().state, State::Idle);
    t.request(Operation::Shutdown, 1).unwrap();
    let before = t.status();
    assert_eq!(t.request(Operation::Shutdown, 2), Err(Error::Unavailable));
    assert_eq!(t.status(), before);
}

#[test]
fn uart_success_and_elapsed_time_never_mean_power_off() {
    for result in [Ok(()), Err(())] {
        let mut t = Transition::new();
        t.request(Operation::Shutdown, 0).unwrap();
        t.quiescing(1);
        t.submitted(result, 2);
        assert!(!t.status().ready);
        t.tick(2 + transition::OBSERVE_MS);
        assert_eq!(t.status().state, State::Uncertain);
        let before = t.status();
        t.abort(Failure::Companion, 10000);
        t.recovered();
        t.submitted(Ok(()), 10001);
        assert_eq!(t.status(), before);
        assert_eq!(
            t.request(Operation::Shutdown, 10002),
            Err(Error::Unavailable)
        );
    }
}

#[test]
fn preparation_failure_requires_completed_recovery_before_readmission() {
    let mut t = Transition::new();
    t.request(Operation::Shutdown, 0).unwrap();
    t.quiescing(1);
    t.abort(Failure::Ant, 2);
    assert_eq!(t.status().state, State::Recovering);
    assert!(!t.status().ready);
    t.submitted(Ok(()), 3);
    assert_eq!(t.status().state, State::Recovering);
    t.recovered();
    assert_eq!(t.status().failure, Some(Failure::Ant));
    assert!(t.status().ready);
    t.request(Operation::Shutdown, 4).unwrap();
    assert_eq!(t.status().sequence, 2);
}

#[test]
fn preparation_and_recovery_deadlines_fail_closed() {
    let mut t = Transition::new();
    t.request(Operation::Shutdown, 0).unwrap();
    t.quiescing(1);
    t.tick(1 + transition::PREPARE_MS);
    assert_eq!(t.status().failure, Some(Failure::PreparationTimeout));
    assert_eq!(t.status().state, State::Recovering);
    t.tick(1 + transition::PREPARE_MS + transition::RECOVER_MS);
    assert_eq!(t.status().state, State::Failed);
    assert_eq!(t.status().failure, Some(Failure::Recovery));
    assert!(!t.status().ready);
    t.recovered();
    assert!(!t.status().ready);
}

#[test]
fn shutdown_is_one_fixed_stock_operation() {
    let frame = transition::shutdown_frame();
    assert_eq!(
        &frame[..14],
        &[0xa5, 12, 0x6f, 0xf1, 2, 16, 0xe2, 2, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        c606_firmware::drivers::companion::crc16(&frame[..14]),
        u16::from_le_bytes([frame[14], frame[15]])
    );
}

#[test]
fn shutdown_gnss_pause_cannot_automatically_reopen_after_diagnostic_lease() {
    use device_api::position_control::State;
    use firmware_services::position_control::Controller;
    let mut gnss = Controller::new();
    gnss.tick(1, 1, Some(1), |_| panic!());
    gnss.suspend(2).unwrap();
    gnss.tick(3, 1, Some(1), |open| {
        assert!(!open);
        Ok(())
    });
    gnss.tick(60_000, 1, Some(1), |_| panic!("must remain suspended"));
    assert_eq!(gnss.snapshot().state, State::SilenceObserved);
    assert_eq!(gnss.snapshot().resume_at_ms, None);
    gnss.resume(60_001).unwrap();
    gnss.tick(60_002, 1, Some(1), |open| {
        assert!(open);
        Ok(())
    });
    assert_eq!(gnss.snapshot().state, State::ResumeSubmitted);
    gnss.tick(60_003, 2, Some(60_003), |_| panic!());
    assert_eq!(gnss.snapshot().state, State::Receiving);
}

#[test]
fn accepted_sound_stop_must_settle_before_recovery_can_succeed() {
    use device_api::position_control::State as G;
    use device_api::power::PeripheralState as P;
    use device_api::sound::State as S;
    let radios = [Some(P::Running); 3];
    for sound in [S::StopQueued, S::StopSubmitted] {
        assert_eq!(
            transition::recovery_status(radios, Some(G::Receiving), Some(sound)),
            P::Pending
        );
    }
    assert_eq!(
        transition::recovery_status(radios, Some(G::Receiving), Some(S::Failed)),
        P::Failed
    );
    assert_eq!(
        transition::recovery_status(radios, Some(G::Receiving), Some(S::Elapsed)),
        P::Running
    );
    assert_eq!(
        transition::recovery_status([None; 3], None, None),
        P::Running
    );
}

#[test]
fn terminal_power_operations_are_bounded_and_reject_extra_arguments() {
    use firmware_console::protocol::Command;
    use firmware_console::protocol::parse;
    assert_eq!(parse(b"CMD 1 POWER").unwrap().command, Command::Power(None));
    assert_eq!(
        parse(b"CMD 2 POWER SHUTDOWN").unwrap().command,
        Command::Power(Some(Operation::Shutdown))
    );
    for command in [
        b"CMD 1 POWER RAW".as_slice(),
        b"CMD 1 POWER SHUTDOWN 7",
        b"CMD 1 POWER STATUS extra",
    ] {
        assert!(parse(command).is_err());
    }
}

#[test]
fn physical_wake_during_charging_preparation_starts_observed_recovery() {
    let mut t = Transition::new();
    t.begin_charging(1);
    t.physical_wake(2);
    assert_eq!(t.status().operation, Some(Operation::Wake));
    assert_eq!(t.status().state, State::Recovering);
    assert!(!t.status().ready);
    t.charging(3);
    t.submitted(Ok(()), 4);
    assert_eq!(t.status().state, State::Recovering);
    t.recovered();
    assert_eq!(t.status().state, State::Completed);
}

#[test]
fn failed_charging_preparation_stays_failed_without_active_rollback() {
    let mut t = Transition::new();
    t.begin_charging(1);
    t.abort(Failure::Sound, 2);
    t.standby_failed();
    t.recovered();
    assert_eq!(t.status().state, State::Failed);
    assert_eq!(t.status().failure, Some(Failure::Sound));
    assert!(!t.status().ready);
}

#[test]
fn charging_startup_stays_quiet_until_an_explicit_wake() {
    let mut t = Transition::new();
    assert_eq!(t.request(Operation::Wake, 0), Err(Error::Unavailable));
    t.begin_charging(1);
    assert_eq!(t.status().state, State::Quiescing);
    assert_eq!(t.status().operation, None);
    assert!(!t.status().ready);
    let preparing = t.status();
    t.submitted(Ok(()), 2);
    assert_eq!(t.status(), preparing);
    assert_eq!(t.request(Operation::Wake, 2), Err(Error::Unavailable));
    t.charging(3);
    let charging = t.status();
    assert_eq!(charging.state, State::Charging);
    assert_eq!(charging.operation, None);
    assert!(!charging.ready);
    t.tick(u64::MAX);
    t.recovered();
    t.begin_charging(4);
    assert_eq!(t.status(), charging);
    assert_eq!(t.request(Operation::Shutdown, 5), Err(Error::Unavailable));
    assert_eq!(t.request(Operation::Sleep, 5), Err(Error::Unavailable));
    t.request(Operation::Wake, 6).unwrap();
    assert_eq!(t.status().sequence, 1);
    assert_eq!(t.status().operation, Some(Operation::Wake));
    assert_eq!(t.status().state, State::Recovering);
    assert_eq!(t.status().at_ms, 6);
    assert_eq!(t.status().failure, None);
    assert!(!t.status().ready);
    assert_eq!(t.wake(7), Err(Error::Unavailable));
    t.tick(7);
    assert_eq!(t.status().state, State::Recovering);
    assert!(!t.status().ready);
    t.recovered();
    assert_eq!(t.status().state, State::Completed);
    assert!(t.status().ready);
}

#[test]
fn submitted_shutdown_cannot_become_charging_or_wake_itself() {
    for result in [Ok(()), Err(())] {
        let mut t = Transition::new();
        t.request(Operation::Shutdown, 0).unwrap();
        t.quiescing(1);
        t.charging(2);
        assert_eq!(t.status().state, State::Quiescing);
        t.submitted(result, 3);
        let submitted = t.status();
        t.begin_charging(4);
        t.charging(4);
        assert_eq!(t.wake(4), Err(Error::Unavailable));
        assert_eq!(t.request(Operation::Wake, 4), Err(Error::Unavailable));
        t.recovered();
        assert_eq!(t.status(), submitted);
        t.tick(3 + transition::OBSERVE_MS);
        assert_eq!(t.status().state, State::Uncertain);
        assert_eq!(t.wake(10_000), Err(Error::Unavailable));
        assert!(!t.status().ready);
    }
}

#[test]
fn wake_recovery_timeout_cannot_report_completion_or_reopen_admission() {
    let mut t = Transition::new();
    t.begin_charging(0);
    t.charging(1);
    t.wake(2).unwrap();
    t.tick(2 + transition::RECOVER_MS);
    assert_eq!(t.status().state, State::Failed);
    assert_eq!(t.status().failure, Some(Failure::Recovery));
    assert!(!t.status().ready);
    t.recovered();
    assert_eq!(t.status().state, State::Failed);
    assert!(!t.status().ready);
    assert_eq!(t.request(Operation::Wake, 50_000), Err(Error::Unavailable));
}

#[test]
fn sleep_requires_observed_wake_and_recovery() {
    for slept in [true, false] {
        let mut t = Transition::new();
        t.request(Operation::Sleep, 0).unwrap();
        t.quiescing(1);
        t.sleeping(2);
        assert_eq!(t.status().state, State::Sleeping);
        t.sleep_returned(slept, 3);
        assert!(!t.status().ready);
        t.recovered();
        assert!(t.status().ready);
        assert_eq!(
            t.status().state,
            if slept {
                State::Completed
            } else {
                State::Failed
            }
        );
    }
}
#[test]
fn uncertain_sleep_stays_gated() {
    let mut t = Transition::new();
    t.request(Operation::Sleep, 0).unwrap();
    t.quiescing(1);
    t.sleeping(2);
    t.sleep_uncertain(3);
    t.sleep_returned(true, 4);
    t.recovered();
    assert_eq!(t.status().state, State::Uncertain);
    assert!(!t.status().ready);
}

#[test]
fn sleep_recovery_requires_each_sensor_to_advance_after_the_boundary() {
    let mut sensors = c606_firmware::drivers::companion_sensors::State::default();
    let motion = |kind| [0xf1, kind, 0, 0, 0, 0, 0, 0];
    let pressure = [0xf1, 3, 0, 0, 0x80, 0x96, 0x98, 0];
    sensors.receive(16, &pressure, 100);
    sensors.receive(16, &motion(1), 100);
    sensors.receive(16, &motion(2), 100);
    assert!(!transition::sensors_after(sensors.snapshot(100, 5000), 100));
    sensors.receive(16, &pressure, 101);
    sensors.receive(16, &motion(1), 101);
    assert!(!transition::sensors_after(sensors.snapshot(101, 5000), 100));
    sensors.receive(16, &motion(2), 101);
    assert!(transition::sensors_after(sensors.snapshot(101, 5000), 100));
    assert!(!transition::sensors_after(
        sensors.snapshot(6000, 5000),
        100
    ));
    sensors.loss();
    assert!(!transition::sensors_after(sensors.snapshot(101, 5000), 100));
}
