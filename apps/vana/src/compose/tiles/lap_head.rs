//! The open lap's head: "LAP TIME" and the lap's number on one baseline,
//! the number on the right in the accent colour, and the lap's time large
//! under them.
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{ACCENT, Align, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct LapHead;

const LADDER: [FaceRole; 2] = [FaceRole::Large, FaceRole::Value];
const NUMBER: FaceRole = FaceRole::Body;
const CAPTION: &[u8] = b"LAP TIME";
/// Rows from the head's baseline to the time's capitals.
const UNDER_HEAD_UNITS: usize = 4;
const BETWEEN_UNITS: usize = 8;

/// Rows above the head's baseline.
fn head(at: &Scaled) -> usize {
    at.face(FaceRole::Caption)
        .fit_height()
        .max(at.face(NUMBER).fit_height())
}

fn number(value: Option<i32>) -> Text<12> {
    let mut text = Text::new();
    let mut lap = Text::<12>::new();
    Metric::LapNumber.write(value, &mut lap);
    let _ = write!(text, "LAP {}", lap.as_str());
    text
}

impl Tile for LapHead {
    type Arg = ();
    const NAME: &'static str = "lap head";

    fn reads((): ()) -> Option<Reads> {
        Some(Reads::Metric(Metric::LapTime))
    }

    fn forms((): (), at: &Scaled) -> Forms {
        let widest = number(Some(i32::MAX));
        let head_width = at.face(FaceRole::Caption).fit_width(CAPTION)
            + at.unit(BETWEEN_UNITS)
            + at.face(NUMBER).fit_width(widest.as_bytes());
        LADDER
            .iter()
            .map(|&role| {
                let face = at.face(role);
                let time = Metric::LapTime
                    .extremes()
                    .iter()
                    .map(|text| face.fit_width(text.as_bytes()))
                    .max()
                    .unwrap_or_default();
                Form {
                    width: head_width.max(time) as u16,
                    height: (head(at) + at.unit(UNDER_HEAD_UNITS) + usize::from(face.cap)) as u16,
                    shapes: 0,
                    runs: 3,
                }
            })
            .collect()
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect;
        let (left, right) = (usize::from(rect.left), usize::from(rect.right));
        let base = usize::from(rect.top) + head(at);
        let lap = number(data.metric(Metric::LapNumber));
        let shown = out.fit(
            &[at.face(NUMBER)],
            lap.as_bytes(),
            right - left,
            right - SPILL,
            base,
            Align::Right,
            ACCENT,
        );
        out.caption(at, CAPTION, shown.left - left, left, base, Align::Left);
        let role = LADDER[usize::from(placed.form)];
        let face = at.face(role);
        let mut time = Text::<12>::new();
        Metric::LapTime.write(data.metric(Metric::LapTime), &mut time);
        out.fit(
            &[face],
            time.as_bytes(),
            right - left,
            left,
            base + at.unit(UNDER_HEAD_UNITS) + usize::from(face.cap),
            Align::Left,
            WHITE,
        );
    }
}
