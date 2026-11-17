use super::*;
use crate::ride::course::bits::{Bits, fields};
extern crate std;
use std::format;
use std::vec::Vec;

const RIGHT: Kind = Kind::Turn(Side::Right);
const LEFT: Kind = Kind::Turn(Side::Left);

fn turn(start_m: f32, end_m: f32, kind: Option<Kind>) -> Turn {
    let mut turn = Turn::default();
    turn.start_mm = (start_m * 1000.0) as u32;
    turn.end_mm = (end_m * 1000.0) as u32;
    turn.kind = kind;
    turn
}

fn corner(start_m: f32, kind: Kind) -> Turn {
    turn(start_m, start_m + 10.0, Some(kind))
}

const LENGTH_MM: u32 = 10_000_000;

fn locate(mm: u32) -> Point {
    (0.0, mm as f32 / 1000.0)
}

struct Rider<'t> {
    nav: Navigator,
    turns: &'t [Turn],
    now_ms: u64,
    speed_mm_s: Option<u32>,
    joined: bool,
    paused: bool,
}

impl<'t> Rider<'t> {
    fn new(turns: &'t [Turn], speed_mm_s: Option<u32>) -> Self {
        Self {
            nav: Navigator::default(),
            turns,
            now_ms: 0,
            speed_mm_s,
            joined: false,
            paused: false,
        }
    }

    fn step(&mut self, observed: Observed) -> Option<Event> {
        self.nav.step(observed, self.turns, locate)
    }

    fn fixed(&self, along_m: f32, off_m: f32, forward: bool) -> MatchedFix {
        let along_mm = (along_m * 1000.0) as u32;
        MatchedFix {
            taken_loop_ms: self.now_ms,
            along_mm,
            to_end_mm: LENGTH_MM.saturating_sub(along_mm),
            off_m,
            here: (off_m, along_m),
            forward,
            speed_mm_s: self.speed_mm_s,
            paused: self.paused,
        }
    }

    fn fix(&mut self, along_m: f32, off_m: f32, forward: bool) -> Option<Event> {
        self.now_ms += 1_000;
        let fixed = self.fixed(along_m, off_m, forward);
        match core::mem::replace(&mut self.joined, true) {
            false => self.step(Observed::Joined(fixed)),
            true => self.step(Observed::Fix(fixed)),
        }
    }

    fn at(&mut self, along_m: f32) -> Option<Event> {
        self.fix(along_m, 0.0, true)
    }

    fn ride(&mut self, from_m: f32, to_m: f32, step_m: f32) -> Vec<(f32, Event)> {
        let mut heard = Vec::new();
        let mut along = from_m;
        while along <= to_m {
            heard.extend(self.at(along).map(|event| (along, event)));
            along += step_m;
        }
        heard
    }

    fn events(&mut self, from_m: f32, to_m: f32, step_m: f32) -> Vec<Event> {
        self.ride(from_m, to_m, step_m)
            .into_iter()
            .map(|(_, event)| event)
            .collect()
    }

    fn sign(&self) -> Sign {
        self.nav.sign(self.turns, self.now_ms)
    }
}

fn shows(glyph: Glyph, metres: u32) -> Sign {
    Sign::Turn {
        glyph,
        distance: Distance::from_mm(metres * 1000),
    }
}

fn chip(glyph: Glyph, metres: u32) -> Sign {
    Sign::Chip {
        glyph,
        distance: Distance::from_mm(metres * 1000),
    }
}

fn opens_at(speed_mm_s: Option<u32>) -> f32 {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, speed_mm_s);
    rider
        .ride(0.0, 1000.0, 1.0)
        .into_iter()
        .find(|&(_, event)| event == Event::TurnSoon)
        .map(|(along, _)| along)
        .unwrap()
}

#[test]
fn the_banner_opens_300_m_ahead_at_any_speed() {
    for speed_mm_s in [
        None,
        Some(0),
        Some(2_778),
        Some(6_944),
        Some(11_111),
        Some(u32::MAX),
    ] {
        assert_eq!(1000.0 - opens_at(speed_mm_s), 300.0, "{speed_mm_s:?}");
    }
}

#[test]
fn the_banner_opens_at_300_m_exactly_and_not_a_millimetre_sooner() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, Some(8_000));
    assert_eq!(rider.at(0.0), Some(Event::Following));
    assert_eq!(rider.at(699.999), None);
    assert_eq!(rider.sign(), chip(Glyph::Right, 300));
    assert_eq!(rider.at(700.0), Some(Event::TurnSoon));
    assert_eq!(rider.sign(), shows(Glyph::Right, 300));
}

#[test]
fn once_open_the_banner_stays_however_the_speed_drops() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, Some(13_000));
    let heard = rider.events(0.0, 700.0, 10.0);
    assert_eq!(heard, [Event::Following, Event::TurnSoon]);
    rider.speed_mm_s = None;
    assert_eq!(rider.events(710.0, 800.0, 10.0), []);
    assert_eq!(rider.sign(), shows(Glyph::Right, 200));
}

#[test]
fn past_the_last_turn_the_banner_holds_it_at_0_m_for_40_m() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 1000.0, 5.0);
    rider.at(1004.0);
    assert_eq!(rider.sign(), shows(Glyph::Right, 0));
    rider.at(1049.9);
    assert_eq!(rider.sign(), shows(Glyph::Right, 0));
    rider.at(1050.0);
    assert_eq!(rider.sign(), chip(Glyph::Finish, 8950));
}

#[test]
fn past_a_turn_the_sign_shows_the_next_at_once_and_stays_up_40_m() {
    let turns = [corner(1000.0, RIGHT), corner(3000.0, LEFT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 1000.0, 5.0);
    rider.at(1004.0);
    assert_eq!(rider.sign(), shows(Glyph::Left, 2000));
    rider.at(1049.0);
    assert_eq!(rider.sign(), shows(Glyph::Left, 1951));
    rider.at(1050.0);
    assert_eq!(rider.sign(), chip(Glyph::Left, 1950));
    let heard = rider.ride(1055.0, 3000.0, 5.0);
    assert_eq!(heard, [(2700.0, Event::TurnSoon), (2970.0, Event::TurnNow)]);
}

fn through_two(turns: &[Turn; 2], speed_mm_s: Option<u32>) -> (Vec<Event>, Vec<Sign>) {
    let mut rider = Rider::new(turns, speed_mm_s);
    rider.ride(0.0, 1000.0, 5.0);
    let (mut heard, mut signs) = (Vec::new(), Vec::new());
    let mut along = 1005.0;
    while along < turns[1].start_mm as f32 / 1000.0 {
        heard.extend(rider.at(along));
        signs.push(rider.sign());
        along += 5.0;
    }
    (heard, signs)
}

#[test]
fn turns_that_open_their_window_by_the_end_of_the_grace_keep_the_banner_up_through_them() {
    for (gap_m, dense) in [(340.0, true), (345.0, false)] {
        let turns = [corner(1000.0, RIGHT), corner(1010.0 + gap_m, LEFT)];
        let (heard, signs) = through_two(&turns, None);
        let up = signs.iter().all(|sign| matches!(sign, Sign::Turn { .. }));
        assert_eq!(up, dense, "{gap_m} m: {signs:?}");
        let want = match dense {
            true => std::vec![Event::TurnNow],
            false => std::vec![Event::TurnSoon, Event::TurnNow],
        };
        assert_eq!(heard, want, "{gap_m} m");
    }
}

#[test]
fn turns_under_25_s_apart_keep_the_banner_up_through_them() {
    for (gap_m, dense) in [(499.0, true), (500.0, false)] {
        let turns = [corner(1000.0, RIGHT), corner(1010.0 + gap_m, LEFT)];
        let (heard, signs) = through_two(&turns, Some(20_000));
        let up = signs.iter().all(|sign| matches!(sign, Sign::Turn { .. }));
        assert_eq!(up, dense, "{gap_m} m: {signs:?}");
        assert_eq!(
            heard.contains(&Event::TurnSoon),
            !dense,
            "{gap_m} m: {heard:?}"
        );
    }
}

#[test]
fn an_unnamed_corner_holds_the_banner_back_until_the_rider_is_past_it() {
    let mut unnamed = turn(900.0, 910.0, None);
    unnamed.cornered = true;
    let gentle = turn(800.0, 810.0, None);
    let turns = [gentle, unnamed, corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 900.0, 5.0);
    assert_eq!(rider.sign(), chip(Glyph::Right, 100));
    let heard = rider.ride(905.0, 1000.0, 5.0);
    assert_eq!(heard, [(905.0, Event::TurnSoon), (970.0, Event::TurnNow)]);
}

#[test]
fn a_gentle_bend_holds_nothing_back() {
    let turns = [turn(900.0, 910.0, None), corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    let heard = rider.ride(0.0, 1000.0, 5.0);
    assert_eq!(
        heard[1..],
        [(700.0, Event::TurnSoon), (970.0, Event::TurnNow)]
    );
}

#[test]
fn round_a_city_block_is_one_turn_the_way_it_goes() {
    let mut block = turn(1000.0, 1045.0, Some(RIGHT));
    block.twice = true;
    let turns = [block];
    let mut rider = Rider::new(&turns, None);
    let heard = rider.events(0.0, 960.0, 5.0);
    assert_eq!(heard, [Event::Following, Event::TurnSoon]);
    assert_eq!(rider.sign(), shows(Glyph::Right, 40));
    assert_eq!(rider.events(965.0, 1035.0, 5.0), [Event::TurnNow]);
    assert_eq!(rider.sign(), shows(Glyph::Right, 0));
}

#[test]
fn the_beep_comes_30_m_before_the_turn_once_however_the_fixes_jitter() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    let mut heard = rider.ride(0.0, 969.0, 1.0);
    for along in [970.0, 968.0, 971.0, 965.0, 975.0, 969.0, 980.0] {
        heard.extend(rider.at(along).map(|event| (along, event)));
    }
    assert_eq!(
        heard,
        [
            (0.0, Event::Following),
            (700.0, Event::TurnSoon),
            (970.0, Event::TurnNow)
        ]
    );
}

#[test]
fn riding_back_30_m_and_on_again_sounds_the_turn_again() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 975.0, 5.0);
    assert_eq!(rider.events(976.0, 980.0, 1.0), []);
    rider.at(951.0);
    assert_eq!(rider.events(955.0, 1000.0, 5.0), [], "29 m back is jitter");
    rider.at(650.0);
    let heard = rider.ride(655.0, 1000.0, 5.0);
    assert_eq!(heard, [(700.0, Event::TurnSoon), (970.0, Event::TurnNow)]);
}

#[test]
fn riding_the_course_backwards_closes_the_banner_and_sounds_nothing() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 900.0, 5.0);
    assert!(matches!(rider.sign(), Sign::Turn { .. }));
    assert_eq!(rider.fix(985.0, 0.0, false), None);
    assert_eq!(rider.sign(), chip(Glyph::Right, 100));
    assert_eq!(rider.at(910.0), None, "it opens again without a sound");
    assert_eq!(rider.sign(), shows(Glyph::Right, 90));
}

fn off_by<'t>(turns: &'t [Turn], fixes: &[f32]) -> (Rider<'t>, Vec<(usize, Event)>) {
    let mut rider = Rider::new(turns, None);
    rider.ride(0.0, 500.0, 5.0);
    let mut heard = Vec::new();
    for (k, &off_m) in fixes.iter().enumerate() {
        heard.extend(rider.fix(500.0, off_m, true).map(|event| (k, event)));
    }
    (rider, heard)
}

#[test]
fn off_course_is_over_40_m_on_fixes_5_s_apart_with_none_nearer_between() {
    let (rider, heard) = off_by(&[], &[41.0; 6]);
    assert_eq!(heard, [(5, Event::OffCourse)]);
    assert!(matches!(rider.sign(), Sign::OffCourse { .. }));
    let (_, heard) = off_by(&[], &[41.0; 5]);
    assert_eq!(heard, [], "4 s");
    let (_, heard) = off_by(&[], &[40.0; 30]);
    assert_eq!(heard, [], "40 m is on");
    let fixes = [41.0, 60.0, 80.0, 39.0, 60.0, 80.0, 90.0, 99.0, 99.0, 99.0];
    let (_, heard) = off_by(&[], &fixes);
    assert_eq!(heard, [(9, Event::OffCourse)], "a fix at 39 m starts again");
}

#[test]
fn a_gap_in_the_fixes_never_confirms_off_course() {
    let turns = [];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 500.0, 5.0);
    assert_eq!(rider.fix(500.0, 60.0, true), None);
    rider.now_ms += 5_000;
    assert_eq!(rider.fix(500.0, 60.0, true), None, "6 s on, after a gap");
    let heard: Vec<_> = (0..5)
        .filter_map(|_| rider.fix(500.0, 60.0, true))
        .collect();
    assert_eq!(heard, [Event::OffCourse]);

    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 500.0, 5.0);
    rider.fix(500.0, 60.0, true);
    assert_eq!(rider.step(Observed::Lost), None);
    assert_eq!(rider.sign(), Sign::None);
    let heard: Vec<_> = (0..5)
        .filter_map(|_| rider.fix(500.0, 60.0, true))
        .collect();
    assert_eq!(heard, [], "the lost fix broke the run");
    assert_eq!(rider.fix(500.0, 60.0, true), Some(Event::OffCourse));
}

#[test]
fn a_lost_fix_keeps_a_confirmed_off_course_and_shows_nothing_meanwhile() {
    let (mut rider, _) = off_by(&[], &[60.0; 6]);
    assert_eq!(rider.step(Observed::Lost), None);
    assert_eq!(rider.sign(), Sign::None);
    assert_eq!(rider.fix(500.0, 60.0, true), None);
    assert!(matches!(rider.sign(), Sign::OffCourse { .. }));
}

#[test]
fn off_course_sounds_again_once_15_s_later() {
    let (_, heard) = off_by(&[], &[60.0; 80]);
    assert_eq!(heard, [(5, Event::OffCourse), (20, Event::StillOff)]);
}

#[test]
fn off_course_holds_however_far_off_until_back_within_25_m_riding_its_way() {
    let mut fixes = std::vec![60.0; 6];
    fixes.extend([500.0, 2_000.0, 10_000.0, 2_000.0, 30.0, 26.0]);
    let (mut rider, heard) = off_by(&[], &fixes);
    assert_eq!(heard, [(5, Event::OffCourse)]);
    assert!(matches!(rider.sign(), Sign::OffCourse { .. }));
    assert_eq!(
        rider.fix(500.0, 20.0, false),
        None,
        "riding it the wrong way"
    );
    assert_eq!(rider.fix(500.0, 25.0, true), Some(Event::OnCourse));
    assert!(matches!(rider.sign(), Sign::Chip { .. }));
}

#[test]
fn the_way_back_leads_to_the_missed_turn_and_never_moves() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 995.0, 5.0);
    for (k, east) in [41.0, 50.0, 60.0, 70.0, 80.0, 90.0].into_iter().enumerate() {
        rider.now_ms += 1_000;
        let mut fixed = rider.fixed(1001.0, 41.0 + k as f32, true);
        fixed.here = (east, 1001.0 + 7.0 * k as f32);
        rider.step(Observed::Fix(fixed));
    }
    let guidance = rider.nav.guidance(&turns, rider.now_ms, locate).unwrap();
    assert_eq!(
        guidance.junction.map(|spot| spot.at),
        Some((0.0, 1005.0)),
        "the corner, midway through the turn's 10 m"
    );
    let back = libm::sqrtf(90.0 * 90.0 + 31.0 * 31.0);
    assert_eq!(
        guidance.sign,
        Sign::OffCourse {
            back: Distance::from_m(back)
        }
    );
}

#[test]
fn far_from_any_turn_the_way_back_leads_to_where_the_rider_left() {
    let turns = [corner(3000.0, RIGHT)];
    let (rider, _) = off_by(&turns, &[45.0, 120.0, 300.0, 500.0, 700.0, 900.0]);
    let guidance = rider.nav.guidance(&turns, rider.now_ms, locate).unwrap();
    assert_eq!(guidance.junction.map(|spot| spot.at), Some((0.0, 500.0)));
    assert_eq!(
        guidance.sign,
        Sign::OffCourse {
            back: Distance::from_m(900.0)
        }
    );
}

fn junction_left_at(along_m: f32) -> Option<Point> {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 1055.0, 5.0);
    rider.at(along_m);
    for _ in 0..6 {
        rider.fix(along_m + 1.0, 50.0, true);
    }
    rider
        .nav
        .guidance(&turns, rider.now_ms, locate)?
        .junction
        .map(|spot| spot.at)
}

#[test]
fn the_way_back_leads_to_a_missed_turn_only_within_60_m_of_where_the_rider_left() {
    assert_eq!(junction_left_at(1060.0), Some((0.0, 1005.0)), "the corner");
    assert_eq!(junction_left_at(1061.0), Some((0.0, 1061.0)));
}

#[test]
fn arriving_sounds_once_and_shows_for_60_s() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 990.0, 5.0);
    let arrived = Observed::Arrived { now_ms: 100_000 };
    assert_eq!(rider.step(arrived), Some(Event::Arrived));
    assert_eq!(rider.step(arrived), None);
    for (now_ms, sign) in [
        (100_000, Sign::Arrived),
        (159_999, Sign::Arrived),
        (160_000, Sign::None),
    ] {
        assert_eq!(rider.nav.sign(&turns, now_ms), sign, "{now_ms}");
    }
}

#[test]
fn of_two_sounds_due_on_one_fix_the_second_waits_a_fix_while_it_holds() {
    let turns = [corner(900.0, RIGHT)];
    let (mut rider, _) = off_by(&turns, &[60.0; 6]);
    assert_eq!(rider.at(800.0), Some(Event::OnCourse));
    assert_eq!(rider.at(805.0), Some(Event::TurnSoon));
    assert_eq!(rider.at(810.0), None);
    let (mut rider, _) = off_by(&turns, &[60.0; 6]);
    assert_eq!(rider.at(880.0), Some(Event::OnCourse));
    assert_eq!(rider.at(881.0), Some(Event::TurnNow));
    assert_eq!(rider.at(882.0), None);
    assert_eq!(rider.sign(), shows(Glyph::Right, 18));
    let (mut rider, _) = off_by(&turns, &[60.0; 6]);
    assert_eq!(rider.at(880.0), Some(Event::OnCourse));
    assert_eq!(rider.fix(881.0, 20.0, false), None);
    assert_eq!(rider.at(882.0), None);
}

#[test]
fn a_beep_put_off_a_fix_is_dropped_once_its_turn_is_passed() {
    let turns = [
        turn(700.0, 710.0, Some(RIGHT)),
        turn(720.0, 730.0, Some(LEFT)),
    ];
    let mut rider = Rider::new(&turns, Some(7_000));
    assert_eq!(
        rider.ride(697.0, 760.0, 7.0),
        [(697.0, Event::Following), (704.0, Event::TurnNow)]
    );
}

#[test]
fn the_same_fix_again_sounds_nothing_more() {
    let turns = [corner(1000.0, RIGHT)];
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 695.0, 5.0);
    rider.now_ms += 1_000;
    let fixed = rider.fixed(700.0, 0.0, true);
    assert_eq!(rider.step(Observed::Fix(fixed)), Some(Event::TurnSoon));
    for _ in 0..3 {
        assert_eq!(rider.step(Observed::Fix(fixed)), None);
    }
    let (mut rider, _) = off_by(&[], &[60.0; 6]);
    let fixed = rider.fixed(500.0, 60.0, true);
    for _ in 0..3 {
        assert_eq!(rider.step(Observed::Fix(fixed)), None);
    }
}

#[test]
fn past_the_last_turn_the_chip_shows_the_course_s_end() {
    let turns = [];
    let mut rider = Rider::new(&turns, None);
    rider.at(2500.0);
    assert_eq!(rider.sign(), chip(Glyph::Finish, 7500));
    assert_eq!(Navigator::default().sign(&turns, 0), Sign::None);
}

#[test]
fn turns_the_list_drops_leave_the_sign_as_it_was() {
    let turns: Vec<Turn> = (0..8)
        .map(|k| corner(100.0 + 200.0 * k as f32, RIGHT))
        .collect();
    let mut rider = Rider::new(&turns, None);
    rider.ride(0.0, 1010.0, 5.0);
    let before = rider.sign();
    let passed = rider.nav.passed();
    assert_eq!(passed, 4);
    rider.nav.forget(passed);
    rider.turns = &turns[passed..];
    assert_eq!(rider.sign(), before);
    let heard = rider.ride(1015.0, 1300.0, 5.0);
    assert_eq!(heard, [(1070.0, Event::TurnNow), (1270.0, Event::TurnNow)]);
}

#[test]
fn distances_are_kept_as_drawn() {
    for (mm, digits, unit) in [
        (0, "0", Unit::Metres),
        (5_000, "0", Unit::Metres),
        (5_001, "10", Unit::Metres),
        (125_000, "120", Unit::Metres),
        (125_001, "130", Unit::Metres),
        (995_000, "990", Unit::Metres),
        (995_001, "1.0", Unit::Kilometres),
        (6_450_000, "6.4", Unit::Kilometres),
        (6_450_001, "6.5", Unit::Kilometres),
        (9_950_000, "9.9", Unit::Kilometres),
        (9_950_001, "10", Unit::Kilometres),
        (12_400_000, "12", Unit::Kilometres),
        (u32::MAX, "4294", Unit::Kilometres),
    ] {
        let distance = Distance::from_mm(mm);
        assert_eq!(
            (format!("{distance}").as_str(), distance.unit()),
            (digits, unit),
            "{mm}"
        );
    }
    assert_eq!(Distance::from_mm(121_000), Distance::from_mm(124_999));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Said {
    Soon(Kind),
    Now(Kind),
}

pub(crate) fn said(event: Event, sign: Sign) -> Option<Said> {
    let Sign::Turn { glyph, .. } = sign else {
        return None;
    };
    let kind = match glyph {
        Glyph::Left => LEFT,
        Glyph::Right => RIGHT,
        Glyph::SharpLeft => Kind::Sharp(Side::Left),
        Glyph::SharpRight => Kind::Sharp(Side::Right),
        Glyph::UTurn => Kind::UTurn,
        Glyph::Finish => return None,
    };
    match event {
        Event::TurnSoon => Some(Said::Soon(kind)),
        Event::TurnNow => Some(Said::Now(kind)),
        _ => None,
    }
}

pub(crate) fn ride_with(
    turns: &[Turn],
    positions: impl IntoIterator<Item = f32>,
) -> Vec<(u32, Said)> {
    let mut rider = Rider::new(turns, None);
    let mut heard = Vec::new();
    let mut last = f32::MIN;
    for metres in positions {
        let on = metres >= last;
        last = metres;
        if !on {
            continue;
        }
        let told = rider.at(metres).and_then(|event| said(event, rider.sign()));
        heard.extend(told.map(|said| (metres as u32, said)));
    }
    heard
}

#[test]
fn a_held_back_turn_now_holds_only_until_its_turn_is_passed() {
    let turns = [corner(500.0, RIGHT), corner(900.0, LEFT)];
    let mut rider = Rider::new(&turns, None);
    let heard = rider.ride(0.0, 480.0, 5.0);
    assert!(
        heard.iter().any(|&(_, event)| event == Event::TurnNow),
        "{heard:?}"
    );
    assert!(rider.nav.still_due(Navigation::TurnNow, &turns));
    rider.ride(485.0, 560.0, 5.0);
    assert!(!rider.nav.still_due(Navigation::TurnNow, &turns));
}

#[test]
fn a_held_back_off_course_holds_only_while_off_course() {
    let mut rider = Rider::new(&[], None);
    rider.ride(0.0, 100.0, 5.0);
    let heard: Vec<_> = (0..7)
        .filter_map(|_| rider.fix(100.0, 60.0, true))
        .collect();
    assert_eq!(heard, [Event::OffCourse]);
    assert!(rider.nav.still_due(Navigation::OffCourse, &[]));
    assert!(!rider.nav.still_due(Navigation::OnCourse, &[]));
    assert_eq!(rider.at(105.0), Some(Event::OnCourse));
    assert!(!rider.nav.still_due(Navigation::OffCourse, &[]));
}

#[test]
fn paused_fixes_never_confirm_or_remind_of_off_course() {
    let mut rider = Rider::new(&[], None);
    rider.ride(0.0, 100.0, 5.0);
    rider.paused = true;
    let heard: Vec<_> = (0..60)
        .filter_map(|_| rider.fix(100.0, 44.0, true))
        .collect();
    assert!(heard.is_empty(), "{heard:?}");
    assert!(!matches!(
        rider.nav.sign(&[], rider.now_ms),
        Sign::OffCourse { .. }
    ));
    rider.paused = false;
    let heard: Vec<_> = (0..7)
        .filter_map(|_| rider.fix(100.0, 44.0, true))
        .collect();
    assert_eq!(heard, [Event::OffCourse], "resumed still off");
    rider.paused = true;
    let heard: Vec<_> = (0..30)
        .filter_map(|_| rider.fix(100.0, 44.0, true))
        .collect();
    assert!(heard.is_empty(), "no reminder while paused: {heard:?}");
    assert!(matches!(
        rider.nav.sign(&[], rider.now_ms),
        Sign::OffCourse { .. }
    ));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Latches {
    pub furthest_mm: u32,
    pub banner: Option<usize>,
    pub last_sounded: Option<usize>,
    pub last_beeped: Option<usize>,
    pub arrived_ms: Option<u64>,
}

pub(crate) fn latches(nav: &Navigator) -> Latches {
    Latches {
        furthest_mm: nav.furthest_mm,
        banner: nav.banner,
        last_sounded: nav.last_sounded,
        last_beeped: nav.last_beeped,
        arrived_ms: nav.arrived_ms,
    }
}

fields! {
    Navigator {
        cursor, furthest_mm, banner, last_sounded, last_beeped, place, last, arrived_ms, lost,
        pending
    }
    MatchedFix { taken_loop_ms, along_mm, to_end_mm, off_m, here, forward, speed_mm_s, paused }
    Spot { along_mm, at }
}

impl Bits for Place {
    fn bits(&self, into: &mut dyn core::hash::Hasher) {
        match *self {
            Place::On => 0u8.bits(into),
            Place::Leaving { since_ms, junction } => {
                1u8.bits(into);
                since_ms.bits(into);
                junction.bits(into);
            }
            Place::Off {
                since_ms,
                junction,
                reminded,
                paused_since_sound,
            } => {
                2u8.bits(into);
                since_ms.bits(into);
                junction.bits(into);
                reminded.bits(into);
                paused_since_sound.bits(into);
            }
        }
    }
}

impl Bits for Raised {
    fn bits(&self, into: &mut dyn core::hash::Hasher) {
        let (tag, TurnId(turn)) = match *self {
            Raised::Following => (0u8, TurnId(0)),
            Raised::TurnSoon { turn } => (1, turn),
            Raised::TurnNow { turn } => (2, turn),
            Raised::OffCourse => (3, TurnId(0)),
            Raised::StillOff => (4, TurnId(0)),
            Raised::OnCourse => (5, TurnId(0)),
        };
        tag.bits(into);
        turn.bits(into);
    }
}
