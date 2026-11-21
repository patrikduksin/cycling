//! The ride's time in each power or heart rate zone, as one bar of the
//! zones' colours beside its caption.
use crate::compose::data::{Data, Effort};
use crate::compose::solve::Placed;
use crate::compose::tile::{Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::ride::summary;
use crate::screens::scaled::Scaled;

pub struct ZoneTime;

impl Tile for ZoneTime {
    type Arg = Effort;
    const NAME: &'static str = "zone time";

    fn forms(_: Effort, at: &Scaled) -> Forms {
        summary::zone_time_forms(at)
    }

    fn prepare(effort: Effort, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if let Some(facts) = data.summary() {
            let heart = effort == Effort::Heart;
            summary::zone_time(heart, placed.rect.rect(), at, &facts, out);
        }
    }
}
