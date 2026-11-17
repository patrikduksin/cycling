use super::phone::{Radio2, Transport, attach};
use super::*;
use crate::app::invariants::Rule;
use crate::app::power::Sheet;
use crate::ride::log::Status;
use crate::ride::recorder::Recorder;
use crate::ride::recorder::invariants as recorder;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::HEART_RATE;
use std::string::ToString;

fn riding() -> Recorder {
    Recorder::in_status(Status::Recording, true)
}

fn recording(mut runtime: Runtime) -> Runtime {
    runtime.ride.internal = riding();
    runtime
}

fn with_phone_open() -> Runtime {
    let mut runtime = booted(Profile::HeartRate);
    attach(&mut runtime);
    runtime.open_phone(PhoneMode::AccessPoint);
    runtime.serve_phone(
        &mut Transport::default(),
        &mut Radio2::default(),
        &mut Absent,
        &mut NoBulk,
        20,
    );
    assert!(runtime.phone_status().is_some());
    runtime
}

fn at_summary() -> Runtime {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.page = Page::Ride;
    radio.connected(HEART_RATE, 0);
    let mut step = |runtime: &mut Runtime, now| {
        tick(runtime, &mut journal, &mut radio, now);
        assert_eq!(runtime.broken(), None, "at {now} ms");
    };
    runtime.input_received(press(Button::BottomRight), 1000, 1000, &mut Radio::new());
    let mut now = 1000;
    while now < 10_000 {
        step(&mut runtime, now);
        now += 250;
    }
    assert!(runtime.ride.internal.riding());
    for (button, at) in [
        (Button::BottomRight, now),
        (Button::TopLeft, now + 400),
        (Button::BottomRight, now + 800),
    ] {
        runtime.input_received(press(button), at, at, &mut Radio::new());
        assert_eq!(runtime.broken(), None, "{button:?} at {at} ms");
        for pass in 0..20 {
            step(&mut runtime, at + pass * 20);
        }
    }
    assert_eq!(runtime.navigation.page, Page::Summary);
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
            runtime.power.sheet = Sheet::Confirm { since: 0 };
        }
        Rule::TwoFtpQuestions => {
            runtime = at_summary();
            runtime.ride.ftp_suggestion = Some(runtime.preferences.profile.ftp_watts + 100);
            runtime.navigation.summary_page =
                crate::screens::ride::summary::pages(runtime.has_elevation());
            assert!(runtime.offer_shown());
            runtime.training.ftp_offer = Some(runtime.preferences.profile.ftp_watts);
        }
        Rule::SavedSensorsDisagree => {
            let peer = device_api::ant::Identity {
                device_type: HEART_RATE,
                device_number: 42,
                transmission_type: 1,
            };
            assert!(runtime.preferences.sensors.select(peer).is_some());
        }
        Rule::SummaryWhileRiding => runtime = recording(at_summary()),
        Rule::AutoLapDisagrees => runtime.break_auto_lap_rule(),
        Rule::StartupStatusDisagreesOnceKnown => {
            runtime.startup_status = device_api::sensors::Startup::Ready.name();
        }
        Rule::TwoRidingRides => {
            runtime.ride.internal = riding();
            runtime.ride.bulk = riding();
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
        internal.ride.internal = Recorder::violating(*rule);
        assert_eq!(
            internal.broken(),
            Some(Broken::Recorder(Medium::Internal, *rule)),
            "{rule}"
        );
        let mut bulk = booted(Profile::HeartRate);
        bulk.ride.bulk = Recorder::violating(*rule);
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
    let mut known = booted(Profile::HeartRate);
    known.startup_status = known.startup.name();
    assert_eq!(known.broken(), None);
}

#[test]
fn a_ride_from_start_to_summary_holds_after_every_step() {
    let runtime = at_summary();
    assert_eq!(runtime.ride.internal.status(), Status::Saved);
}
