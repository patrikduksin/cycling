use super::*;
use crate::ride::homing::Reported;
use device_api::gps::Snapshot;
use firmware_services::gps::Parser;
use std::format;
use std::string::String;

#[derive(Default)]
struct Receiver(Parser);

impl Receiver {
    fn fix(&mut self, (east, north): (f32, f32), now: u64) {
        let (lat, lon) = at(east, north);
        let seconds = now / 1_000;
        let body = format!(
            "GNGGA,{:02}{:02}{:02}.00,{},{},1,08,0.9,10.0,M,0.0,M,,",
            seconds / 3_600,
            seconds / 60 % 60,
            seconds % 60,
            angle(lat, 2, ['N', 'S']),
            angle(lon, 3, ['E', 'W']),
        );
        let sum = body.bytes().fold(0, |sum, b| sum ^ b);
        for byte in format!("${body}*{sum:02X}\r\n").bytes() {
            self.0.push(byte, now);
        }
    }
}

fn angle(e7: i32, degrees: usize, [positive, negative]: [char; 2]) -> String {
    let e7_abs = u64::from(e7.unsigned_abs());
    let minutes_e7 = e7_abs % 10_000_000 * 60;
    format!(
        "{:0degrees$}{:02}.{:07},{}",
        e7_abs / 10_000_000,
        minutes_e7 / 10_000_000,
        minutes_e7 % 10_000_000,
        if e7 < 0 { negative } else { positive },
    )
}

impl Rider<'_> {
    fn hear(&mut self, gps: &Snapshot, now: u64) -> Option<Event> {
        let fix = self.homing.observe(Reported::fresh(gps, now), now);
        self.follower.observe(fix, now, None)
    }
}

const PASS_MS: u64 = 250;

#[derive(Clone, Copy)]
struct Pass {
    now: u64,
    event: Option<Event>,
    sign: Sign,
}

fn off_at(off_ms: &[u64]) -> Vec<Pass> {
    let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 512]);
    let mut rider = Rider::new(&mut crumbs, &mut line, &mut []);
    let course = course_of(&[(0.0, 0.0), (0.0, 1000.0)]);
    rider.set_course(&course);
    let mut receiver = Receiver::default();
    let mut heard = Vec::new();
    for now in (PASS_MS..=30_000).step_by(PASS_MS as usize) {
        match now {
            ..=10_000 if now % 1_000 == 0 => receiver.fix((0.0, now as f32 / 100.0), now),
            _ if off_ms.contains(&now) => receiver.fix((60.0, 100.0), now),
            _ => {}
        }
        let event = rider.hear(&receiver.0.snapshot(now), now);
        if rider.needs_course() {
            rider.set_course(&course);
        }
        heard.push(Pass {
            now,
            event,
            sign: rider.follower.sign(now),
        });
    }
    heard
}

fn events(heard: &[Pass]) -> Vec<(u64, Event)> {
    heard
        .iter()
        .filter_map(|pass| pass.event.map(|event| (pass.now, event)))
        .collect()
}

#[test]
fn off_course_is_confirmed_by_a_receiver_fix_5_s_later_never_by_its_last_position_again() {
    let heard = off_at(&[11_000, 13_000]);
    assert!(
        matches!(events(&heard)[..], [(_, Event::Following)]),
        "two fixes 2 s apart"
    );
    let gone = heard
        .iter()
        .find(|pass| pass.now > 13_000 && pass.sign == Sign::None)
        .map(|pass| pass.now);
    assert_eq!(gone, Some(18_250), "lost 5 s after the last fix");

    let heard = off_at(&[11_000, 13_000, 16_000]);
    assert_eq!(events(&heard)[1..], [(16_000, Event::OffCourse)]);

    let standing: Vec<u64> = (11..=16).map(|s| s * 1_000).collect();
    let heard = off_at(&standing);
    assert_eq!(
        events(&heard)[1..],
        [(16_000, Event::OffCourse)],
        "standing still off it"
    );
}
