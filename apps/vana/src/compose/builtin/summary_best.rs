//! The summary's best power page: the ride's best 5 s, 1, 5 and 20 min as
//! bars under their header, then its climbs and laps.
use super::summary_overview::{CELLS_BOTTOM, FINISH, Keep, MARGIN_UNITS, cells, page};
use super::{Builtin, Nodes};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{BEST_EFFORT, WORDS};
use crate::ride::totals::EFFORT_SECONDS;
use crate::screens::ride::summary::{Stat, Words};

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| tree(),
    buttons: FINISH,
};

pub(super) fn tree() -> Nodes {
    let mut body = Nodes::new();
    let _ = body.push(Node::leaf(WORDS, Words::BestPower as u8, Size::CONTENT));
    for index in 0..EFFORT_SECONDS.len() as u8 {
        let _ = body.push(Node::leaf(BEST_EFFORT, index, Size::CONTENT));
    }
    let _ = body.extend_from_slice(&cells(Stat::Climbs, Stat::Laps));
    let children = 2 + EFFORT_SECONDS.len() as u8;
    let keep = Keep {
        top: 0,
        bottom: CELLS_BOTTOM,
    };
    page(MARGIN_UNITS, keep, children, &body)
}
