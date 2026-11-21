//! The course chip's room: the next turn and how far, which the sign's own
//! painter draws in the box the tree gives it. Over it and the time above,
//! the banner shows a turn.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;
use crate::screens::sign;

pub struct Chip;

/// The chip's rows.
pub const CHIP_UNITS: usize = 28;

impl Tile for Chip {
    type Arg = ();
    const NAME: &'static str = "chip";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: sign::chip_width(at) as u16,
            height: at.unit(CHIP_UNITS) as u16,
            shapes: 0,
            runs: 0,
        }])
    }

    fn prepare((): (), _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}
}
