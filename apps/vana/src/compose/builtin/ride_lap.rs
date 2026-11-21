//! The open lap: its time and number, its distance and averages, the last
//! finished lap with how this one compares, and a bar per lap filling the
//! rest of the page. The lap's numbers share their room as even gaps.
use super::ride_stats::{RIDE_PAGE, page, pair};
use super::{Builtin, Choices};
use crate::compose::data::Metric;
use crate::compose::node::{Node, Size};
use crate::compose::registry::{LAP_HEAD, LAPS};

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| page(2, body()),
    buttons: RIDE_PAGE,
});

const PAIRS: [[Metric; 2]; 4] = [
    [Metric::LapDistance, Metric::LapPower],
    [Metric::LapSpeed, Metric::LapHeart],
    [Metric::PrevLapTime, Metric::PrevLapPower],
    [Metric::SpeedDiff, Metric::PowerDiff],
];

fn body() -> impl Iterator<Item = Node> {
    let grow = Size::grow(1);
    let numbers = [
        Node::col(1 + PAIRS.len() as u8, grow),
        Node::leaf(LAP_HEAD, 0, Size::CONTENT),
    ];
    let pairs = PAIRS.into_iter().flat_map(pair);
    numbers
        .into_iter()
        .chain(pairs)
        .chain([Node::leaf(LAPS, 0, grow)])
}
