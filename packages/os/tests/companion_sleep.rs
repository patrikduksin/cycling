#[allow(dead_code)]
#[path = "../src/device/companion_sleep.rs"]
mod companion_sleep;
use companion_sleep::{Action, Sleep, State};
use cycling_os::{
    capabilities::Observation,
    companion_sensors::{Motion, Pressure, Snapshot, State as Sensors},
};
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
        ..Sensors::default().snapshot(at, 5000)
    }
}
fn response(class: u8, payload: [u8; 8]) -> [u8; 16] {
    let mut frame = cycling_os::companion::command(16, payload);
    frame[4] = class;
    let crc = cycling_os::companion::crc16(&frame[..14]);
    frame[14..].copy_from_slice(&crc.to_le_bytes());
    frame
}
fn started() -> Sleep {
    let mut sleep = Sleep::new();
    assert!(sleep.begin(sample(10), 10, true));
    assert_eq!(sleep.take_action(), Some(Action::Suspend));
    sleep.submitted(Action::Suspend, Ok(()), 11);
    sleep
}
fn ack(sleep: &mut Sleep, at: u64) {
    sleep.receive(&response(5, [0xe2, 2, 1, 0, 0, 0, 0, 0]), at);
}
#[test]
fn frames_and_submissions_are_fixed_and_never_replayed() {
    assert_eq!(
        &Action::Suspend.frame()[6..14],
        &[0xe2, 2, 0, 0, 0, 3, 0, 0]
    );
    assert_eq!(&Action::Resume.frame()[6..14], &[0xe2, 2, 0, 0, 0, 7, 0, 0]);
    let mut sleep = started();
    assert_eq!(sleep.take_action(), None);
    assert!(!sleep.begin(sample(12), 12, true));
    sleep.observe(sample(10), 4000, true);
    assert_eq!(sleep.state(), State::Uncertain);
    assert_eq!(sleep.take_action(), None);
}
#[test]
fn generic_ack_requires_correct_class_and_quiet_acquisition() {
    let mut sleep = started();
    sleep.receive(&response(4, [0xe2, 2, 1, 0, 0, 0, 0, 0]), 12);
    sleep.observe(sample(10), 511, true);
    assert_eq!(sleep.state(), State::Suspending);
    ack(&mut sleep, 512);
    sleep.observe(sample(500), 513, true);
    sleep.observe(sample(500), 1012, true);
    assert_eq!(sleep.state(), State::Suspending);
    sleep.observe(sample(500), 1013, true);
    assert_eq!(sleep.state(), State::Suspended);
    sleep.observe(sample(1014), 1014, true);
    assert_eq!(sleep.state(), State::Uncertain);
}
#[test]
fn explicit_recovery_after_uncertain_suspend_is_single_and_observed() {
    let mut sleep = Sleep::new();
    assert!(sleep.begin(sample(10), 10, true));
    let action = sleep.take_action().unwrap();
    sleep.submitted(action, Err(()), 11);
    assert_eq!(sleep.state(), State::Uncertain);
    assert!(!sleep.request_resume(sample(10), 12, false));
    assert!(sleep.request_resume(sample(10), 13, true));
    assert_eq!(sleep.take_action(), Some(Action::Resume));
    sleep.submitted(Action::Resume, Err(()), 14);
    sleep.observe(sample(15), 15, true);
    assert_eq!(sleep.state(), State::Uncertain);
    sleep.observe(sample(16), 16, true);
    assert_eq!(sleep.state(), State::Ready);
    assert_eq!(sleep.take_action(), None);
    assert!(!sleep.request_resume(sample(16), 17, true));
}
#[test]
fn one_stuck_sensor_cannot_complete_recovery() {
    let mut sleep = started();
    ack(&mut sleep, 12);
    sleep.observe(sample(10), 511, true);
    assert!(sleep.request_resume(sample(10), 512, true));
    sleep.take_action();
    sleep.submitted(Action::Resume, Ok(()), 513);
    sleep.observe(sample(514), 514, true);
    let mut partial = sample(515);
    partial.motion[1] = sample(514).motion[1];
    sleep.observe(partial, 515, true);
    assert_eq!(sleep.state(), State::Resuming);
    sleep.observe(partial, 8513, true);
    assert_eq!(sleep.state(), State::Failed);
}
#[test]
fn button_resume_is_passive_and_missing_motion_stays_failed() {
    let mut sleep = started();
    ack(&mut sleep, 12);
    sleep.observe(sample(10), 511, true);
    sleep.receive(&response(4, [0x49, 1, 0, 0, 0, 0, 1, 0x80]), 512);
    assert_eq!(sleep.state(), State::Uncertain);
    assert!(sleep.request_resume(sample(10), 513, true));
    assert_eq!(sleep.take_action(), None);
    let mut partial = sample(520);
    partial.motion[1] = Observation::Unavailable;
    sleep.observe(partial, 520, true);
    sleep.observe(partial, 8513, true);
    assert_eq!(sleep.state(), State::Failed);
}

#[test]
fn fresh_partial_autonomous_resume_never_sends_initialization() {
    let mut sleep = started();
    ack(&mut sleep, 12);
    sleep.observe(sample(10), 511, true);
    let mut partial = sample(512);
    partial.motion = [Observation::Unavailable; 2];
    assert!(sleep.request_resume(partial, 513, true));
    assert_eq!(sleep.state(), State::Resuming);
    assert_eq!(sleep.take_action(), None);
    sleep.observe(sample(514), 514, true);
    assert_eq!(sleep.state(), State::Resuming);
    sleep.observe(sample(515), 515, true);
    assert_eq!(sleep.state(), State::Ready);
    assert_eq!(sleep.take_action(), None);
}

#[test]
fn cached_reports_before_quiet_do_not_imply_autonomous_resume() {
    let mut sleep = started();
    ack(&mut sleep, 12);
    sleep.observe(sample(100), 100, true);
    sleep.observe(sample(100), 600, true);
    assert_eq!(sleep.state(), State::Suspended);
    assert!(sleep.request_resume(sample(100), 601, true));
    assert_eq!(sleep.take_action(), Some(Action::Resume));
}
