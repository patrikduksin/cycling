//! Zeroing the saved power meter through the runtime and a radio fake.
use super::*;
use crate::screens::sensors::CalibrationNote;
use device_api::ant::SendStage;

const POWER: u8 = 11;

fn power_meter() -> Identity {
    Identity {
        device_type: POWER,
        device_number: 1234,
        transmission_type: 1,
    }
}

fn run(runtime: &mut Runtime, radio: &mut Radio, from: u64, to: u64) {
    for now in (from..to).step_by(10) {
        radio.channels.tick(now);
        advance(runtime, radio, now);
    }
}

/// A runtime with the power meter saved, connected and sending power pages.
fn live() -> (Runtime, Radio) {
    let mut runtime = restored(&[power_meter()]);
    let mut radio = Radio::new();
    run(&mut runtime, &mut radio, 0, 100);
    radio.data(POWER, [0x10, 1, 0xff, 80, 0, 0, 200, 0], 100);
    run(&mut runtime, &mut radio, 100, 200);
    (runtime, radio)
}

fn sends(radio: &Radio) -> usize {
    let request = crate::sensors::calibration::request();
    radio
        .requests
        .iter()
        .filter(|op| matches!(op, AntOperation::Send { data, .. } if *data == request))
        .count()
}

fn note(runtime: &Runtime) -> Option<CalibrationNote> {
    runtime.calibration.note()
}

#[test]
fn a_zero_request_is_sent_once_and_the_offset_is_shown() {
    let (mut runtime, mut radio) = live();
    runtime.calibrate(&mut radio, 200);
    assert_eq!(note(&runtime), Some(CalibrationNote::Calibrating));
    run(&mut runtime, &mut radio, 200, 400);
    assert_eq!(sends(&radio), 1);

    radio.data(POWER, [0x01, 0xac, 0xff, 0xff, 0xff, 0xff, 0x00, 0x02], 400);
    run(&mut runtime, &mut radio, 400, 500);
    let done = note(&runtime).unwrap();
    assert_eq!(done, CalibrationNote::Done(512));
    assert_eq!(done.text().as_str(), "CALIBRATED  OFFSET 512");
}

#[test]
fn power_pages_keep_flowing_while_it_waits() {
    let (mut runtime, mut radio) = live();
    runtime.calibrate(&mut radio, 200);
    radio.data(POWER, [0x10, 2, 0xff, 80, 0, 0, 210, 0], 250);
    run(&mut runtime, &mut radio, 200, 300);
    assert_eq!(note(&runtime), Some(CalibrationNote::Calibrating));
}

#[test]
fn a_failure_page_ends_it() {
    let (mut runtime, mut radio) = live();
    runtime.calibrate(&mut radio, 200);
    run(&mut runtime, &mut radio, 200, 400);
    radio.data(POWER, [0x01, 0xaf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], 400);
    run(&mut runtime, &mut radio, 400, 500);
    assert_eq!(note(&runtime), Some(CalibrationNote::Failed));
}

#[test]
fn silence_ends_in_a_failure_after_the_bound() {
    let (mut runtime, mut radio) = live();
    runtime.calibrate(&mut radio, 200);
    for now in (200..10_100).step_by(10) {
        radio.channels.tick(now);
        if now % 250 == 0 {
            radio.data(POWER, [0x10, 3, 0xff, 80, 0, 0, 200, 0], now);
        }
        advance(&mut runtime, &mut radio, now);
    }
    assert_eq!(note(&runtime), Some(CalibrationNote::Calibrating));
    for now in (10_100..10_300).step_by(10) {
        radio.channels.tick(now);
        if now % 250 == 0 {
            radio.data(POWER, [0x10, 3, 0xff, 80, 0, 0, 200, 0], now);
        }
        advance(&mut runtime, &mut radio, now);
    }
    assert_eq!(note(&runtime), Some(CalibrationNote::Failed));
}

#[test]
fn an_answer_with_nothing_asked_is_ignored() {
    let (mut runtime, mut radio) = live();
    radio.data(POWER, [0x01, 0xac, 0xff, 0xff, 0xff, 0xff, 0x00, 0x02], 200);
    run(&mut runtime, &mut radio, 200, 300);
    assert_eq!(note(&runtime), None);
}

#[test]
fn without_a_live_power_meter_nothing_is_sent() {
    let mut runtime = restored(&[power_meter()]);
    let mut radio = Radio::new();
    run(&mut runtime, &mut radio, 0, 100);
    runtime.calibrate(&mut radio, 100);
    assert_eq!(note(&runtime), Some(CalibrationNote::NotConnected));
    run(&mut runtime, &mut radio, 100, 300);
    assert_eq!(sends(&radio), 0);
}

#[test]
fn a_bridge_that_rejects_the_message_fails_it() {
    let (mut runtime, mut radio) = live();
    runtime.calibrate(&mut radio, 200);
    let (id, _, _) = radio.channels.pending_send().unwrap();
    radio.channels.send_submitted(id, 300);
    radio.channels.send_reply(id, false, 310);
    assert_eq!(
        radio.channels.send_status(id).unwrap().stage,
        SendStage::BridgeReplied { accepted: false }
    );
    run(&mut runtime, &mut radio, 310, 400);
    assert_eq!(note(&runtime), Some(CalibrationNote::Failed));
}
