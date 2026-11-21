use crate::compose::data::{Data, Effort};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, zone_color};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct ZoneChip;

const FACES: [FaceRole; 2] = [FaceRole::Body, FaceRole::Caption];

impl Arg for Effort {
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

impl Tile for ZoneChip {
    type Arg = Effort;
    const NAME: &'static str = "zone";

    fn forms(_: Effort, at: &Scaled) -> Forms {
        FACES
            .iter()
            .map(|&role| Form::text(at.face(role), b"Z7"))
            .collect()
    }

    fn prepare(effort: Effort, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let face = at.face(FACES[usize::from(placed.form)]);
        let zone = data.zone(effort);
        let mut text = Text::<12>::new();
        let _ = match zone {
            Some(zone) => write!(text, "Z{zone}"),
            None => text.write_str("Z-"),
        };
        let rect = placed.rect;
        out.fit(
            &[face],
            text.as_bytes(),
            usize::from(rect.width()),
            usize::from(rect.left),
            usize::from(rect.bottom),
            Align::Left,
            zone_color(zone.unwrap_or(0)),
        );
    }
}
