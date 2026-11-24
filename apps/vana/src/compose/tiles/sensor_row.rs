//! A sensor list's item on its card: the sensor's icon, its name over the
//! line under it, and on NEARBY a plus at the right; or ADD SENSOR.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Run, Shape, on_baseline};
use crate::screens::chrome::{Align, GREEN, Line, SECONDARY, WHITE};
use crate::screens::icons::{Icon, Sensor, Size, StaticIcon};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{Item, Selected, kind, widest};
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct SensorRow;

/// Columns inside the card before the icon and after the text or plus.
const INSET_UNITS: usize = 6;
/// The icon's column, the text starting after it.
const ICON_UNITS: usize = 32;
/// The plus's column at NEARBY's right, and the columns before it.
const PLUS_UNITS: usize = 26;
/// From the name's baseline to the baseline of the line under it.
const LINES_UNITS: usize = 16;
/// Columns between a state and the dot after it.
const DOT_GAP_UNITS: usize = 5;
/// The dot's radius, in quarter pixels.
const DOT_UNITS: usize = 6;
const HEIGHT_UNITS: usize = 40;

impl Tile for SensorRow {
    type Arg = ();
    const NAME: &'static str = "sensor_row";

    /// A card fits beside the icon the widest name over the widest state
    /// or, on NEARBY, the widest identity and the plus.
    fn forms((): (), at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        let name = widest::name(caption);
        let saved = name.max(widest::state(caption));
        let saved = saved.max(at.face(FaceRole::Body).fit_width(b"ADD SENSOR"));
        let identity = caption.fit_width(b"ID ") + widest::identity(caption);
        let found = name.max(identity).max(caption.fit_width(b"NOT SEEN"));
        // Units round apart, so these add up as `prepare` places them.
        let before = at.unit(INSET_UNITS) + at.unit(ICON_UNITS);
        let saved = before + saved + at.unit(INSET_UNITS);
        let found = before + found + at.unit(INSET_UNITS + PLUS_UNITS);
        forms(&[Form {
            width: saved.max(found) as u16,
            height: at.unit(HEIGHT_UNITS) as u16,
            shapes: 2,
            runs: 3,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let (Some(row), Some(menu)) = (data.row(), data.sensors()) else {
            return;
        };
        let Some(item) = menu.item(row.list, row.index) else {
            return;
        };
        let rect = placed.rect.rect();
        let body = at.face(FaceRole::Body);
        let left = rect.left + at.unit(INSET_UNITS);
        let text = left + at.unit(ICON_UNITS);
        let middle = |size: usize| rect.top + (rect.height() - size) / 2;
        let icon = |out: &mut Canvas, icon: &'static Icon, left: usize, color: u16| {
            let top = middle(usize::from(icon.size));
            out.shape(Shape::Icon {
                icon: StaticIcon(icon),
                left,
                top,
                color,
            });
        };
        let name_base =
            middle(usize::from(body.cap) + at.unit(LINES_UNITS)) + usize::from(body.cap);
        let under = name_base + at.unit(LINES_UNITS);
        let faces = [body, at.face(FaceRole::Caption)];
        match item {
            Item::Add => {
                icon(out, at.art.plus, left, GREEN);
                let base = middle(usize::from(body.cap)) + usize::from(body.cap);
                let width = rect.right - at.unit(INSET_UNITS) - text;
                out.fit(
                    &[body],
                    b"ADD SENSOR",
                    width,
                    text,
                    base,
                    Align::Left,
                    WHITE,
                );
            }
            Item::Saved(selected) => {
                let sensor = Sensor::of(selected.identity.device_type);
                icon(out, at.art.sensor(sensor, Size::Row), left, sensor.tint());
                let width = rect.right - at.unit(INSET_UNITS) - text;
                let name = kind(selected.identity.device_type);
                out.fit(&faces, name, width, text, name_base, Align::Left, WHITE);
                state(out, at, selected, text, under, width);
            }
            Item::Found {
                identity, present, ..
            } => {
                let sensor = Sensor::of(identity.device_type);
                icon(out, at.art.sensor(sensor, Size::Small), left, SECONDARY);
                let plus = rect.right - at.unit(INSET_UNITS) - usize::from(at.art.plus.size);
                icon(out, at.art.plus, plus, GREEN);
                let width = rect.right - at.unit(INSET_UNITS + PLUS_UNITS) - text;
                let name = kind(identity.device_type);
                out.fit(&faces, name, width, text, name_base, Align::Left, WHITE);
                let mut line = Text::<24>::new();
                if present {
                    let _ = write!(line, "ID {}", menu.identity_label(identity).as_str());
                } else {
                    let _ = line.push_str("NOT SEEN");
                }
                out.caption(at, line.as_bytes(), width, text, under, Align::Left);
            }
        }
    }
}

/// A saved sensor's state under its name and, once its battery wants
/// changing, the warning after a dot; a live sensor whose warning does
/// not fit beside LIVE shows the warning alone.
fn state(
    out: &mut Canvas,
    at: &Scaled,
    selected: Selected,
    left: usize,
    base: usize,
    width: usize,
) {
    let caption = at.face(FaceRole::Caption);
    let (text, color) = selected.link();
    let fits = |first: &[u8], second: &[u8]| {
        caption.ink_width(first) + 2 * at.unit(DOT_GAP_UNITS) + caption.width(second) + SPILL
            <= width
    };
    let (first, second) = match selected.battery_warning() {
        Some(warning) if selected.live() && !fits(text, warning.0) => (warning, None),
        Some(warning) if selected.live() => ((text, warning.1), Some(warning)),
        warning => ((text, color), warning),
    };
    let shown: Run = out.fit(&[caption], first.0, width, left, base, Align::Left, first.1);
    let Some((text, color)) = second.filter(|&(second, _)| fits(shown.line.text(), second)) else {
        return;
    };
    let gap = at.unit(DOT_GAP_UNITS);
    let end = shown.left + caption.ink_width(shown.line.text());
    out.shape(Shape::Disc {
        cx: 4 * (end + gap) as i32 + 2,
        cy: 4 * base as i32 - 2 * i32::from(caption.cap),
        radius: at.unit(DOT_UNITS) as i32,
        color,
    });
    let line = Line::new(caption, text);
    out.run(on_baseline(line, end + 2 * gap, base, Align::Left, color));
}
