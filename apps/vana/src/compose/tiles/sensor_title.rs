//! A sensor list's title: SENSORS, or the trouble it names in amber, and
//! NEARBY.
use crate::compose::data::{Data, ListKind};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::widest;
use firmware_shell::rendering::font::FaceRole;

pub struct SensorTitle;

const FACES: [FaceRole; 2] = [FaceRole::Title, FaceRole::Body];

impl Tile for SensorTitle {
    type Arg = ListKind;
    const NAME: &'static str = "sensor_title";

    /// A look fits the list's name in its face and, in the body face it
    /// falls back to, any trouble SENSORS names in its place.
    fn forms(list: ListKind, at: &Scaled) -> Forms {
        let body = at.face(FaceRole::Body);
        let trouble = match list {
            ListKind::Sensors => widest::title(body),
            _ => 0,
        };
        FACES
            .iter()
            .map(|&role| {
                let name = Form::text(at.face(role), list.name().as_bytes());
                Form {
                    width: name.width.max(trouble as u16),
                    ..name
                }
            })
            .collect()
    }

    fn prepare(list: ListKind, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let (text, color) = match list {
            ListKind::Sensors => menu.title(),
            _ => (list.name(), WHITE),
        };
        let rect = placed.rect;
        let faces = &FACES[usize::from(placed.form)..];
        let faces = [at.face(faces[0]), at.face(FACES[1])];
        out.fit(
            &faces,
            text.as_bytes(),
            usize::from(rect.width()),
            usize::from(rect.left),
            usize::from(rect.bottom),
            Align::Left,
            color,
        );
    }
}
