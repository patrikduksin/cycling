use crate::compose::data::{Data, Metric};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct Caption;

impl Tile for Caption {
    type Arg = Metric;
    const NAME: &'static str = "caption";

    fn forms(metric: Metric, at: &Scaled) -> Forms {
        let face = at.face(FaceRole::Caption);
        forms(&[Form::text(face, metric.name().as_bytes())])
    }

    fn prepare(metric: Metric, placed: &Placed, at: &Scaled, _: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect;
        out.caption(
            at,
            metric.name().as_bytes(),
            usize::from(rect.width()),
            usize::from(rect.left),
            usize::from(rect.bottom),
            Align::Left,
        );
    }
}
