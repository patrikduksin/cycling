#[path = "../src/device/power_transition.rs"]
mod transition;
use cycling_os::{
    capabilities::Error,
    power::{Failure, Operation, State},
};
use transition::{Gate, Transition};

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
    use std::sync::{Arc, Barrier};
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
fn unsupported_sleep_and_duplicate_request_do_not_change_shutdown() {
    let mut t = Transition::new();
    assert_eq!(t.request(Operation::Sleep, 0), Err(Error::Unsupported));
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
        cycling_os::companion::crc16(&frame[..14]),
        u16::from_le_bytes([frame[14], frame[15]])
    );
}

#[test]
fn shutdown_gnss_pause_cannot_automatically_reopen_after_diagnostic_lease() {
    use cycling_os::position_control::{Controller, State};
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
    use cycling_os::{
        position_control::State as G, power::PeripheralState as P, sound::State as S,
    };
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
    use cycling_os::terminal_protocol::{Command, parse};
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
