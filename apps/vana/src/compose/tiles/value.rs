use crate::compose::data::{Data, Metric};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct Value;

const FACES: [FaceRole; 4] = [
    FaceRole::Hero,
    FaceRole::Large,
    FaceRole::Value,
    FaceRole::Body,
];

impl Arg for Metric {
    fn decode(raw: u8) -> Option<Self> {
        Self::from_id(raw)
    }

    fn encode(self) -> u8 {
        self.id()
    }

    fn label(self) -> &'static str {
        self.name()
    }
}

impl Tile for Value {
    type Arg = Metric;
    const NAME: &'static str = "value";

    fn forms(metric: Metric, at: &Scaled) -> Forms {
        let extremes = metric.extremes();
        FACES
            .iter()
            .map(|&role| {
                let face = at.face(role);
                let widest = extremes
                    .iter()
                    .map(Text::as_bytes)
                    .max_by_key(|text| face.width(text))
                    .unwrap_or_default();
                Form::text(face, widest)
            })
            .collect()
    }

    fn prepare(metric: Metric, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let face = at.face(FACES[usize::from(placed.form)]);
        let mut text = Text::<12>::new();
        metric.write(data.metric(metric), &mut text);
        let rect = placed.rect;
        out.fit(
            &[face],
            text.as_bytes(),
            usize::from(rect.width()),
            usize::from(rect.right) - SPILL,
            usize::from(rect.bottom),
            Align::Right,
            metric.color(),
        );
    }
}
