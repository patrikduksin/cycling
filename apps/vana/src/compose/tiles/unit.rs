use crate::compose::data::{Data, Metric};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, SECONDARY};
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct Unit;

const FACES: [FaceRole; 2] = [FaceRole::Body, FaceRole::Caption];

impl Tile for Unit {
    type Arg = Metric;
    const NAME: &'static str = "unit";

    fn forms(metric: Metric, at: &Scaled) -> Forms {
        FACES
            .iter()
            .map(|&role| Form::text(at.face(role), metric.unit().as_bytes()))
            .collect()
    }

    fn prepare(metric: Metric, placed: &Placed, at: &Scaled, _: &dyn Data, out: &mut Canvas) {
        let face = at.face(FACES[usize::from(placed.form)]);
        let rect = placed.rect;
        out.fit(
            &[face],
            metric.unit().as_bytes(),
            usize::from(rect.width()),
            usize::from(rect.left),
            usize::from(rect.bottom),
            Align::Left,
            SECONDARY,
        );
    }
}
