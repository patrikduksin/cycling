//! REPLACE SENSOR's choice: an arrow over the saved sensor on the left
//! and the one that would replace it on the right, each its device number
//! (with its transmission type where another peer shares the number) over
//! CURRENT or NEW.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, Align, WHITE};
use crate::screens::icons::StaticIcon;
use crate::screens::scaled::Scaled;
use crate::screens::sensors::widest;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct SensorSwap;

const SIDE_UNITS: usize = 92;
const BETWEEN_UNITS: usize = 12;
/// Columns a side's number and transmission type keep clear of its edges.
const KEPT_UNITS: usize = 4;
/// Rows from the arrow's top to the numbers' baseline.
const NUMBER_UNITS: usize = 72;
/// Rows from the numbers' baseline to the transmission types'.
const TRANSMISSION_UNITS: usize = 17;
/// Rows from the transmission types' baseline to the labels'.
const LABEL_UNITS: usize = 16;

/// A side's columns: room for the widest device number, with the columns
/// it keeps clear, its transmission type and its label.
fn side(at: &Scaled) -> usize {
    let (caption, body) = (at.face(FaceRole::Caption), at.face(FaceRole::Body));
    let number = body.fit_width(b"") + widest::digits(body, 5) + at.unit(KEPT_UNITS);
    let transmission = caption.fit_width(b"T") + widest::digits(caption, 3) + at.unit(KEPT_UNITS);
    let label = caption.fit_width(b"CURRENT");
    let sides = [at.unit(SIDE_UNITS), number, transmission, label];
    sides.into_iter().max().unwrap_or(0)
}

impl Tile for SensorSwap {
    type Arg = ();
    const NAME: &'static str = "sensor_swap";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: (2 * side(at) + at.unit(BETWEEN_UNITS)) as u16,
            height: at.unit(NUMBER_UNITS + TRANSMISSION_UNITS + LABEL_UNITS) as u16,
            shapes: 1,
            runs: 6,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let Some((new, current)) = menu.replacing() else {
            return;
        };
        let rect = placed.rect.rect();
        let arrow = at.art.arrow;
        out.shape(Shape::Icon {
            icon: StaticIcon(arrow),
            left: rect.center_x() - usize::from(arrow.size) / 2,
            top: rect.top,
            color: ACCENT,
        });
        let side = side(at);
        let whole = 2 * side + at.unit(BETWEEN_UNITS);
        let left = rect.left + rect.width().saturating_sub(whole) / 2;
        let number = rect.top + at.unit(NUMBER_UNITS);
        let transmission = number + at.unit(TRANSMISSION_UNITS);
        let label = transmission + at.unit(LABEL_UNITS);
        let faces = [at.face(FaceRole::Value), at.face(FaceRole::Body)];
        let caption = at.face(FaceRole::Caption);
        let sides = [(current, &b"CURRENT"[..]), (Some(new), b"NEW")];
        for (at_side, (peer, name)) in sides.into_iter().enumerate() {
            let center = left + at_side * (side + at.unit(BETWEEN_UNITS)) + side / 2;
            out.caption(at, name, side, center, label, Align::Center);
            let Some(peer) = peer else { continue };
            let mut digits = Text::<6>::new();
            let _ = write!(digits, "{}", peer.device_number);
            let width = side - at.unit(KEPT_UNITS);
            out.fit(
                &faces,
                digits.as_bytes(),
                width,
                center,
                number,
                Align::Center,
                WHITE,
            );
            if menu.shares_number(peer) {
                let mut kind = Text::<4>::new();
                let _ = write!(kind, "T{}", peer.transmission_type);
                let text = kind.as_bytes();
                out.fit(
                    &[caption],
                    text,
                    width,
                    center,
                    transmission,
                    Align::Center,
                    WHITE,
                );
            }
        }
    }
}
