//! The VANA mark the reveal draws: the wordmark inside one `ACCENT` ring,
//! closing clockwise from 12 o'clock while LABS fades in.
use crate::compose::data::{Data, PageData};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::brand::{self, DONE, RING_UNITS, SQUARE_UNITS};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Rect;
use crate::screens::scaled::Scaled;

pub struct Mark;

/// The pixels the ring's edge blends past its stroke.
const RING_BLEND: usize = 1;

impl Tile for Mark {
    type Arg = ();
    const NAME: &'static str = "mark";

    fn forms((): (), at: &Scaled) -> Forms {
        let side = (at.unit(SQUARE_UNITS + RING_UNITS) + 2 * RING_BLEND) as u16;
        forms(&[Form {
            width: side,
            height: side,
            shapes: 1,
            runs: 2,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let progress = match data.page() {
            PageData::Brand(progress) => progress,
            _ => DONE,
        };
        let rect = placed.rect.rect();
        let side = at.unit(SQUARE_UNITS);
        let (left, top) = (
            rect.center_x() - side / 2,
            rect.top + (rect.height() - side) / 2,
        );
        let square = Rect {
            left,
            top,
            right: left + side,
            bottom: top + side,
        };
        brand::reveal(out, at, square, progress);
    }
}
