use super::data::{Effort, Metric};
use super::node::{Flags, Node, Size, When};
use super::parse::Screen;
use super::registry::{GAUGE, ICON, SPACE, UNIT, VALUE, ZONE};
use super::reject::Reject;
use super::solve::Solved;
use super::tiles::icon::Glyph;
use crate::screens::scaled::Scaled;
use core::fmt::Write;
use core::hint::black_box;

const fn tile(component: u8, arg: u8) -> Node {
    Node::leaf(component, arg, Size::CONTENT)
}

const fn band(children: u8) -> Node {
    Node::row(children, Size::CONTENT)
}

pub const MAIN: [Node; 26] = [
    Node::col(7, Size::grow(1)),
    Node::leaf(SPACE, 0, Size::units(22)),
    band(2),
    tile(VALUE, Metric::Elapsed as u8),
    tile(VALUE, Metric::Grade as u8),
    band(2).when(When::new(Flags::COURSE, Flags::NONE)),
    tile(ICON, Glyph::Arrow as u8),
    tile(VALUE, Metric::Distance as u8),
    band(2),
    tile(VALUE, Metric::Speed as u8).hero(),
    tile(UNIT, Metric::Speed as u8),
    Node::col(2, Size::CONTENT),
    band(2),
    tile(VALUE, Metric::Power as u8),
    tile(ZONE, Effort::Power as u8),
    tile(GAUGE, Effort::Power as u8),
    Node::col(2, Size::CONTENT),
    band(2),
    tile(VALUE, Metric::Heart as u8),
    tile(ZONE, Effort::Heart as u8),
    tile(GAUGE, Effort::Heart as u8),
    band(4).when(When::new(Flags::NONE, Flags::ARRIVED)),
    tile(VALUE, Metric::Cadence as u8),
    tile(UNIT, Metric::Cadence as u8),
    tile(VALUE, Metric::Distance as u8),
    tile(UNIT, Metric::Distance as u8),
];

pub const MAIN_BYTES: [u8; MAIN.len() * 4] = {
    let mut bytes = [0; MAIN.len() * 4];
    let mut i = 0;
    while i < MAIN.len() {
        let node = MAIN[i].bytes();
        let mut j = 0;
        while j < 4 {
            bytes[i * 4 + j] = node[j];
            j += 1;
        }
        i += 1;
    }
    bytes
};

pub const COMBINATIONS: [Flags; 4] = [
    Flags::NONE,
    Flags::COURSE,
    Flags::ARRIVED,
    Flags::COURSE.with(Flags::ARRIVED),
];

pub const ROUNDS: u32 = 100;

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub accept_ns: u64,
    pub solve_ns: u64,
    pub leaves: usize,
}

/// Time `MAIN` on `at`, reading microseconds from `now`.
#[inline(never)]
pub fn measure(at: &Scaled, now: &dyn Fn() -> u64) -> Result<Timing, Reject> {
    let started = now();
    for _ in 0..ROUNDS {
        black_box(Screen::parse(0, black_box(&MAIN_BYTES), at, None))?;
    }
    let accept_ns = (now() - started) * 1000 / u64::from(ROUNDS);
    let screen = Screen::parse(0, &MAIN_BYTES, at, None)?;
    let mut solved = Solved::EMPTY;
    let started = now();
    for _ in 0..ROUNDS {
        for flags in COMBINATIONS {
            screen.solve(black_box(flags), &mut solved);
            black_box(&solved);
        }
    }
    let solves = u64::from(ROUNDS) * COMBINATIONS.len() as u64;
    let solve_ns = (now() - started) * 1000 / solves;
    Ok(Timing {
        accept_ns,
        solve_ns,
        leaves: solved.leaves().len(),
    })
}

pub fn report(at: &Scaled, now: &dyn Fn() -> u64, out: &mut dyn Write) {
    let _ = match measure(at, now) {
        Ok(timing) => write!(
            out,
            "compose nodes={} combinations={} rounds={ROUNDS} accept_ns={} solve_ns={} leaves={}",
            MAIN.len(),
            COMBINATIONS.len(),
            timing.accept_ns,
            timing.solve_ns,
            timing.leaves,
        ),
        Err(reject) => write!(out, "compose refused: {reject}"),
    };
}
