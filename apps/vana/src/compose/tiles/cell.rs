//! One of the summary's numbers: a caption over its value, as the ride
//! pages show a cell.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::ride::summary::{self, Stat};
use crate::screens::scaled::Scaled;

pub struct Cell;

impl Arg for Stat {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        core::str::from_utf8(self.caption()).unwrap_or("")
    }
}

impl Tile for Cell {
    type Arg = Stat;
    const NAME: &'static str = "cell";

    fn forms(stat: Stat, at: &Scaled) -> Forms {
        summary::cell_forms(stat, at)
    }

    fn prepare(stat: Stat, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if let Some(facts) = data.summary() {
            summary::cell(stat, placed.rect.rect(), at, &facts, out);
        }
    }
}
