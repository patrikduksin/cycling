//! The ride's elevation profile. It grows into the rows its screen leaves;
//! the chart is drawn row by row by its own painter, charted for the box
//! the tree gives it.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::ride::summary;
use crate::screens::scaled::Scaled;

pub struct Profile;

impl Tile for Profile {
    type Arg = ();
    const NAME: &'static str = "profile";

    fn forms((): (), at: &Scaled) -> Forms {
        summary::profile_forms(at)
    }

    fn prepare((): (), _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}

    fn heavy() -> bool {
        true
    }
}
