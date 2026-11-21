//! The map: the ground the track and the course draw on. It grows into every
//! row its screen leaves, and its own painter fills the ground row by row; the
//! tile writes only what the map says in words.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, WHITE};
use crate::screens::ride::map::Words;
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct Map;

const NO_TRACK: &[u8] = b"NO TRACK";
const NO_GPS: &[u8] = b"NO GPS YET";
const STARTS: &[u8] = b"MAP STARTS WITH GPS";
const DRAWS: &[u8] = b"DRAWS AS YOU RIDE";
/// The note under NO GPS YET, below its baseline.
const NOTE_UNITS: usize = 24;
/// DRAWS AS YOU RIDE, above the rider.
const ABOVE_RIDER_UNITS: usize = 36;
/// The fewest rows a map is worth riding by.
const ROWS_UNITS: usize = 120;

impl Tile for Map {
    type Arg = ();
    const NAME: &'static str = "map";

    fn forms((): (), at: &Scaled) -> Forms {
        let (body, caption) = (at.face(FaceRole::Body), at.face(FaceRole::Caption));
        let width = [NO_TRACK, NO_GPS]
            .map(|text| body.fit_width(text))
            .into_iter()
            .chain([STARTS, DRAWS].map(|text| caption.fit_width(text)))
            .max()
            .unwrap_or_default();
        forms(&[Form {
            width: width as u16,
            height: at.unit(ROWS_UNITS) as u16,
            shapes: 0,
            runs: 2,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let (width, cx) = (rect.width(), (rect.left + rect.right) / 2);
        let middle = (rect.top + rect.bottom) / 2;
        let say = |out: &mut Canvas, text: &[u8]| {
            let face = at.face(FaceRole::Body);
            out.fit(&[face], text, width, cx, middle, Align::Center, WHITE);
        };
        match data.map().and_then(|map| map.words()) {
            None => say(out, NO_TRACK),
            Some(Words::NoGps) => {
                say(out, NO_GPS);
                let below = middle + at.unit(NOTE_UNITS);
                out.caption(at, STARTS, width, cx, below, Align::Center);
            }
            Some(Words::DrawsAsYouRide { rider }) => {
                let above = rider.saturating_sub(at.unit(ABOVE_RIDER_UNITS));
                out.caption(at, DRAWS, width, cx, above, Align::Center);
            }
            Some(Words::None) => {}
        }
    }

    fn heavy() -> bool {
        true
    }
}
