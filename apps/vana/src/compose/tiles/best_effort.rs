//! One of the ride's best efforts: its length, a bar as long as its share
//! of the highest, and its watts.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Forms, Tile};
use crate::ride::totals::EFFORT_SECONDS;
use crate::screens::canvas::Canvas;
use crate::screens::ride::summary;
use crate::screens::scaled::Scaled;

pub struct BestEffort;

/// Which best effort, by its place in `EFFORT_SECONDS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Index(pub u8);

impl Arg for Index {
    fn decode(raw: u8) -> Option<Self> {
        (usize::from(raw) < EFFORT_SECONDS.len()).then_some(Self(raw))
    }

    fn encode(self) -> u8 {
        self.0
    }
}

impl Tile for BestEffort {
    type Arg = Index;
    const NAME: &'static str = "best effort";

    fn forms(_: Index, at: &Scaled) -> Forms {
        summary::effort_forms(at)
    }

    fn prepare(index: Index, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if let Some(facts) = data.summary() {
            let rect = placed.rect.rect();
            summary::effort(usize::from(index.0), rect, at, &facts, out);
        }
    }
}
