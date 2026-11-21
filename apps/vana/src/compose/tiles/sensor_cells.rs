//! A saved sensor's BATTERY and ID side by side, each caption over its
//! value: the voltage in the colour of its urgency, or the status word
//! when the sensor sends none.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, on_baseline};
use crate::screens::chrome::{AMBER, Align, Line, RED, SECONDARY, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::widest;
use crate::sensors::ant::BatteryStatus;
use firmware_shell::rendering::font::FaceRole;

pub struct SensorCells;

const BATTERY_UNITS: usize = 84;
const BETWEEN_UNITS: usize = 8;
const ID_UNITS: usize = 104;
/// From the captions' baseline to the values'.
const VALUE_UNITS: usize = 34;
/// Columns between a voltage and its unit.
const UNIT_GAP_UNITS: usize = 6;

/// The two cells' columns: room for the widest voltage, battery word and
/// identity, in the face each falls back to.
fn cells(at: &Scaled) -> (usize, usize) {
    let (caption, body) = (at.face(FaceRole::Caption), at.face(FaceRole::Body));
    let value = at.face(FaceRole::Value);
    let statuses = [
        BatteryStatus::New,
        BatteryStatus::Good,
        BatteryStatus::Ok,
        BatteryStatus::Low,
        BatteryStatus::Critical,
    ];
    let words = widest::of(body, statuses.map(|status| status.text().as_bytes()));
    let volts = value.fit_width(b".") + widest::digits(value, 3);
    let volts = volts + at.unit(UNIT_GAP_UNITS) + caption.fit_width(b"V");
    let battery = [
        at.unit(BATTERY_UNITS),
        words,
        volts,
        caption.fit_width(b"BATTERY"),
    ];
    let id = at.unit(ID_UNITS).max(widest::identity(body));
    (battery.into_iter().max().unwrap_or(0), id)
}

impl Tile for SensorCells {
    type Arg = ();
    const NAME: &'static str = "sensor_cells";

    fn forms((): (), at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        let (battery, id) = cells(at);
        forms(&[Form {
            width: (battery + at.unit(BETWEEN_UNITS) + id) as u16,
            height: (caption.fit_height() + at.unit(VALUE_UNITS)) as u16,
            shapes: 0,
            runs: 5,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let Some(shown) = menu.shown() else {
            return;
        };
        let rect = placed.rect.rect();
        let caption = at.face(FaceRole::Caption);
        let (battery, id) = cells(at);
        let whole = battery + at.unit(BETWEEN_UNITS) + id;
        let left = rect.left + rect.width().saturating_sub(whole) / 2;
        let id_left = left + battery + at.unit(BETWEEN_UNITS);
        let captions = rect.top + caption.fit_height();
        let values = rect.bottom;
        out.caption(at, b"BATTERY", battery, left, captions, Align::Left);
        out.caption(at, b"ID", id, id_left, captions, Align::Left);
        let value = at.face(FaceRole::Value);
        let body = at.face(FaceRole::Body);
        let label = menu.identity_label(shown.identity);
        let faces = [value, body];
        out.fit(
            &faces,
            label.as_bytes(),
            id,
            id_left,
            values,
            Align::Left,
            WHITE,
        );
        let Some(reported) = shown.battery else {
            let line = Line::new(value, b"--");
            out.run(on_baseline(line, left, values, Align::Left, WHITE));
            return;
        };
        let color = match reported.status {
            BatteryStatus::Critical => RED,
            status if status.is_low() => AMBER,
            _ => WHITE,
        };
        let Some(volts) = shown.volts() else {
            let word = reported.status.text().as_bytes();
            out.run(on_baseline(
                Line::new(body, word),
                left,
                values,
                Align::Left,
                color,
            ));
            return;
        };
        let line = Line::new(value, volts.as_bytes());
        out.run(on_baseline(line, left, values, Align::Left, color));
        let unit_left = left + value.ink_width(volts.as_bytes()) + at.unit(UNIT_GAP_UNITS);
        let unit = Line::new(caption, b"V");
        out.run(on_baseline(unit, unit_left, values, Align::Left, SECONDARY));
    }
}
