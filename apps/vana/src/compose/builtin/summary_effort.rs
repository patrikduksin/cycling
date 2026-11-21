//! The summary's effort page: normalized and peak power, work and the
//! calories it burned, average and peak heart rate, the time in each power
//! and heart rate zone and, once measured, the heart rate recovery.
use super::summary_overview::{CELLS_TOP, FINISH, Keep, MARGIN_UNITS, cells, page};
use super::{Builtin, Nodes};
use crate::compose::data::Effort;
use crate::compose::node::{Node, Size};
use crate::compose::registry::{CELL, ZONE_TIME};
use crate::screens::ride::summary::Stat;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| tree(),
    buttons: FINISH,
};

/// Its last tile, the recovery, is wide enough to lose a bottom corner: the
/// page keeps only the done cue's box clear.
const DONE_CUE_UNITS: u8 = 30;

pub(super) fn tree() -> Nodes {
    let mut body = Nodes::new();
    for (left, right) in [
        (Stat::Np, Stat::MaxPower),
        (Stat::Work, Stat::Kcal),
        (Stat::AvgHeart, Stat::MaxHeart),
    ] {
        let _ = body.extend_from_slice(&cells(left, right));
    }
    let _ = body.extend_from_slice(&[
        Node::leaf(ZONE_TIME, Effort::Power.id(), Size::CONTENT),
        Node::leaf(ZONE_TIME, Effort::Heart.id(), Size::CONTENT),
        Node::leaf(CELL, Stat::Recovery as u8, Size::CONTENT),
    ]);
    let keep = Keep {
        top: CELLS_TOP,
        bottom: DONE_CUE_UNITS,
    };
    page(MARGIN_UNITS, keep, 6, &body)
}
