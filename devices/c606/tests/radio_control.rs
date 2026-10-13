use c606_firmware::drivers::radio_control::Control;
use device_api::connectivity::ControlError;
use device_api::connectivity::Operation;
use device_api::observation::Error;
use device_api::power::PeripheralState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Scan,
    Connect,
}

fn ready() -> Control<Op> {
    let mut control = Control::new();
    control.set_ready();
    control
}

#[test]
fn requests_are_refused_until_the_owner_is_ready() {
    let mut control = Control::new();
    assert_eq!(control.request(Op::Scan), Err(Error::Unavailable));
    assert_eq!(control.power_request(true), Err(Error::Unavailable));
    assert_eq!(control.status().sequence, 0);
    control.set_ready();
    assert_eq!(control.request(Op::Scan), Ok(()));
}

#[test]
fn one_pending_command_is_delivered_once_and_never_replayed() {
    let mut control = ready();
    control.request(Op::Scan).unwrap();
    let status = control.status();
    assert_eq!((status.sequence, status.operation), (1, Operation::Pending));
    assert_eq!(status.error, None);
    // A second command waits for completion, not behind the first.
    assert_eq!(control.request(Op::Connect), Err(Error::Unavailable));
    assert_eq!(control.take_command(), Some(Op::Scan));
    assert_eq!(control.take_command(), None);
    // Still pending until the owner completes it.
    assert_eq!(control.request(Op::Connect), Err(Error::Unavailable));
    control.finish(false, Some(ControlError::ScanFailed));
    let status = control.status();
    assert_eq!(status.operation, Operation::Failed);
    assert_eq!(status.error_name(), "scan_failed");
    control.request(Op::Connect).unwrap();
    assert_eq!(control.status().sequence, 2);
    assert_eq!(control.status().error_name(), "none");
}

#[test]
fn automatic_or_superseded_operations_do_not_report_completion() {
    let mut control = ready();
    control.finish(true, Some(ControlError::PeerNotFound));
    assert_eq!(control.status().operation, Operation::Idle);
    control.request(Op::Scan).unwrap();
    control.finish(false, None);
    // Command still unclaimed: a newer request owns the status.
    assert_eq!(control.status().operation, Operation::Pending);
    control.take_command();
    control.finish(false, None);
    assert_eq!(control.status().operation, Operation::Completed);
}

#[test]
fn suspend_waits_for_pending_work_and_blocks_new_commands() {
    let mut control = ready();
    control.request(Op::Scan).unwrap();
    assert_eq!(control.power_request(true), Err(Error::Unavailable));
    control.take_command();
    assert_eq!(control.power_request(true), Err(Error::Unavailable));
    control.complete(None);
    assert_eq!(control.power_request(true), Ok(true));
    assert!(control.suspended());
    assert_eq!(control.power(), PeripheralState::Pending);
    assert_eq!(control.request(Op::Scan), Err(Error::Unavailable));
    // Repeating the same target is accepted without restarting the transition.
    assert_eq!(control.power_request(true), Ok(false));
    control.power_complete(true, control.suspend_outcome(true));
    assert_eq!(control.power(), PeripheralState::Quiescent);
    assert_eq!(control.power_request(true), Ok(false));
    assert_eq!(control.power(), PeripheralState::Quiescent);
}

#[test]
fn resume_reaches_running_and_reopens_admission() {
    let mut control = ready();
    control.power_request(true).unwrap();
    control.power_complete(true, PeripheralState::Quiescent);
    assert_eq!(control.power_request(false), Ok(true));
    assert_eq!(control.request(Op::Scan), Err(Error::Unavailable));
    assert_eq!(control.complete_resume(), PeripheralState::Running);
    control.request(Op::Scan).unwrap();
}

#[test]
fn superseded_power_results_are_ignored() {
    let mut control = ready();
    control.power_request(true).unwrap();
    control.power_request(false).unwrap();
    control.power_complete(true, PeripheralState::Quiescent);
    assert_eq!(control.power(), PeripheralState::Pending);
    assert_eq!(control.complete_resume(), PeripheralState::Running);
}

#[test]
fn uncertainty_is_never_cleared_and_fails_every_transition() {
    let mut control = ready();
    control.mark_uncertain();
    control.power_request(true).unwrap();
    assert_eq!(control.suspend_outcome(true), PeripheralState::Failed);
    control.power_complete(true, control.suspend_outcome(true));
    control.power_request(false).unwrap();
    assert_eq!(control.complete_resume(), PeripheralState::Failed);
    assert_eq!(control.request(Op::Scan), Err(Error::Unavailable));
    control.power_request(true).unwrap();
    control.power_request(false).unwrap();
    assert_eq!(control.complete_resume(), PeripheralState::Failed);
    assert!(control.uncertain());
}

#[test]
fn stopped_owner_fails_closed() {
    let mut control = ready();
    control.request(Op::Connect).unwrap();
    control.stop(ControlError::RunnerStopped);
    assert!(!control.ready());
    assert_eq!(control.take_command(), None);
    assert_eq!(control.power(), PeripheralState::Failed);
    assert_eq!(control.status().operation, Operation::Failed);
    assert_eq!(control.status().error_name(), "runner_stopped");
    assert_eq!(control.suspend_outcome(true), PeripheralState::Failed);
    assert_eq!(control.request(Op::Scan), Err(Error::Unavailable));
    assert_eq!(control.power_request(true), Err(Error::Unavailable));
}
