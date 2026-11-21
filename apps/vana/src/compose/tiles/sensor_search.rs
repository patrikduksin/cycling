//! NEARBY's search line: how the search goes, and the refresh mark at its
//! end, lit while a search runs. A tap on it searches again.
use crate::compose::data::Data;
use crate::compose::input::{Action, OnPage};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, Align, WHITE};
use crate::screens::icons::StaticIcon;
use crate::screens::scaled::Scaled;
use crate::screens::sensors::widest;
use firmware_shell::rendering::font::FaceRole;

pub struct SensorSearch;

/// Columns between the line and the mark.
const GAP_UNITS: usize = 4;
/// What the line says besides a message.
const LINES: [&[u8]; 2] = [b"NO SENSORS FOUND", b"FIND NEARBY"];

impl Tile for SensorSearch {
    type Arg = ();
    const NAME: &'static str = "sensor_search";

    fn forms((): (), at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        let mark = usize::from(at.art.refresh.size);
        let line = widest::message(caption).max(widest::of(caption, LINES));
        forms(&[Form {
            width: (line + at.unit(GAP_UNITS) + mark) as u16,
            height: caption.fit_height().max(mark) as u16,
            shapes: 1,
            runs: 1,
        }])
    }

    /// A tap searches again, so accept keeps the line out of a list's card,
    /// where a tap would land on the card.
    fn action((): ()) -> Option<Action> {
        Some(Action::Page(OnPage::Search))
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let rect = placed.rect.rect();
        let caption = at.face(FaceRole::Caption);
        let mark = at.art.refresh;
        let size = usize::from(mark.size);
        let middle = rect.top + rect.height() / 2;
        let base = middle + usize::from(caption.cap) / 2;
        let (line, color) = menu.search_line();
        let width = rect.width() - size - at.unit(GAP_UNITS);
        out.fit(
            &[caption],
            line.as_bytes(),
            width,
            rect.left,
            base,
            Align::Left,
            color,
        );
        out.shape(Shape::Icon {
            icon: StaticIcon(mark),
            left: rect.right - size,
            top: middle - size / 2,
            color: if menu.scanning() { ACCENT } else { WHITE },
        });
    }
}
