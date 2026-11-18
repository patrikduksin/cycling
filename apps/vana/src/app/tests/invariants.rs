use super::phone::{Radio2, Transport, attach};
use super::*;
use crate::app::invariants::Rule;
use crate::app::navigation::To;
use crate::app::power::Sheet;
use crate::ride::log::Status;
use crate::ride::recorder::Recorder;
use crate::ride::recorder::invariants as recorder;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::HEART_RATE;
use std::string::ToString;

fn riding() -> Recorder {
    Recorder::running()
}

fn recording(mut runtime: Runtime) -> Runtime {
    *runtime.ride.internal_mut() = riding();
    runtime
}

fn with_phone_open() -> Runtime {
    let mut runtime = booted(Profile::HeartRate);
    attach(&mut runtime);
    runtime.open_phone(PhoneMode::AccessPoint);
    runtime.serve_phone(
        &mut Transport::default(),
        &mut Radio2::default(),
        &mut MustNotWrite,
        &mut MustNotWrite,
        20,
    );
    assert!(runtime.phone_status().is_some());
    runtime
}

fn at_summary() -> Runtime {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.go(To::Page(Page::Ride));
    radio.connected(HEART_RATE, 0);
    let mut step = |runtime: &mut Runtime, now| {
        tick(runtime, &mut journal, &mut radio, now);
        assert_eq!(runtime.broken(), None, "at {now} ms");
    };
    runtime.input_received(press(Role::Act), 1000, 1000, &mut Radio::new());
    let mut now = 1000;
    while now < 10_000 {
        step(&mut runtime, now);
        now += 250;
    }
    assert!(runtime.ride.internal().riding());
    for (button, at) in [
        (Role::Act, now),
        (Role::Back, now + 400),
        (Role::Act, now + 800),
    ] {
        runtime.input_received(press(button), at, at, &mut Radio::new());
        assert_eq!(runtime.broken(), None, "{button:?} at {at} ms");
        for pass in 0..20 {
            step(&mut runtime, at + pass * 20);
        }
    }
    assert_eq!(runtime.navigation.page(), Page::Summary);
    runtime
}

fn violating(rule: Rule) -> Runtime {
    let mut runtime = booted(Profile::HeartRate);
    match rule {
        Rule::PhoneWhileRecording => runtime = recording(with_phone_open()),
        Rule::TurningOffWhileRecording => {
            runtime = recording(runtime);
            runtime.power.sheet = Sheet::TurningOff {
                started: None,
                progress: 0,
                drawn: false,
                failed: 0,
                due: false,
            };
        }
        Rule::ConfirmOffWhileRecording => {
            runtime = recording(runtime);
            runtime.power.sheet = Sheet::Confirm {
                since: 0,
                unsaved: 0,
            };
        }
        Rule::SummaryWhileRiding => runtime = recording(at_summary()),
        Rule::HostTokenOwned => runtime
            .ride
            .set_host(crate::ride::current::Request::Pending(9)),
        Rule::PageWhileWiping => {
            runtime.format = crate::app::format::Format::Running {
                stage: crate::app::format::Stage::SettleIntent,
                scope: crate::app::format::Scope::one(crate::app::storage::Store::Internal),
                resumed: false,
                clear: None,
            };
            runtime.navigation.go(To::Page(Page::Home));
        }
        Rule::EditingOffSettings => runtime.navigation.edit_off_settings(),
        Rule::TwoRidingRides => {
            *runtime.ride.internal_mut() = riding();
            *runtime.ride.bulk_mut() = riding();
        }
        Rule::TurningOffUnsynced => {
            runtime.ride.set_volatile();
            *runtime.ride.internal_mut() = riding();
            runtime.protect_open_ride();
            *runtime.ride.internal_mut() = Recorder::default();
            runtime.power.sheet = crate::app::power::TURNING_OFF;
        }
        Rule::UnprotectedVolatileRide => {
            runtime.ride.set_volatile();
            *runtime.ride.internal_mut() = riding();
        }
        Rule::VolatileError => {
            runtime.ride.set_volatile();
            *runtime.ride.internal_mut() = Recorder::failed();
        }
    }
    runtime
}

#[test]
fn each_cross_owner_rule_is_broken_by_its_own_state() {
    for rule in Rule::ALL {
        assert_eq!(
            violating(*rule).broken(),
            Some(Broken::Rule(*rule)),
            "{rule}"
        );
    }
}

#[test]
fn a_broken_recorder_on_either_medium_names_its_medium() {
    for rule in recorder::Rule::ALL {
        let mut internal = booted(Profile::HeartRate);
        *internal.ride.internal_mut() = Recorder::violating(*rule);
        assert_eq!(
            internal.broken(),
            Some(Broken::Recorder(Medium::Internal, *rule)),
            "{rule}"
        );
        let mut bulk = booted(Profile::HeartRate);
        *bulk.ride.bulk_mut() = Recorder::violating(*rule);
        assert_eq!(
            bulk.broken(),
            Some(Broken::Recorder(Medium::Bulk, *rule)),
            "{rule}"
        );
    }
    assert_eq!(
        Broken::Recorder(Medium::Bulk, recorder::Rule::BatchOverflow).to_string(),
        "bulk recorder: sample batch overflows its buffer"
    );
}

#[test]
fn consistent_runtimes_hold() {
    assert_eq!(booted(Profile::HeartRate).broken(), None);
    assert_eq!(with_phone_open().broken(), None);
    assert_eq!(recording(booted(Profile::HeartRate)).broken(), None);
}

#[test]
fn a_ride_from_start_to_summary_holds_after_every_step() {
    let runtime = at_summary();
    assert_eq!(runtime.ride.internal().status(), Status::Saved);
}

#[test]
fn a_test_sheet_covers_the_summary_offer_until_it_is_answered() {
    use crate::workout::ftp::Source;
    let mut runtime = at_summary();
    let current = runtime.preferences.profile.ftp_watts;
    runtime
        .training
        .ftp
        .set(Source::RideSummary, Some(current + 100));
    runtime
        .navigation
        .go(To::SummaryPage(crate::screens::ride::summary::pages(
            runtime.has_elevation(),
        )));
    assert!(runtime.offer_shown());
    runtime
        .training
        .ftp
        .set(Source::WorkoutTest, Some(current + 10));
    assert!(!runtime.offer_shown());
    assert_eq!(runtime.ftp_sheet(), Some((current + 10, current)));
    runtime.input_received(press(Role::Back), 100_000, 100_000, &mut Radio::new());
    assert_eq!(runtime.ftp_sheet(), None);
    assert!(runtime.offer_shown());
    assert_eq!(runtime.preferences.profile.ftp_watts, current);
}
