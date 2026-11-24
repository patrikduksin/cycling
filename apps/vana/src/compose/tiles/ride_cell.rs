//! A metric's value under a caption that names it and its unit, as the
//! ride's totals and laps list them: the caption, then the value, on the
//! left or centred. A difference is green when it gained.
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct RideCell;

const LADDER: [FaceRole; 3] = [FaceRole::Large, FaceRole::Value, FaceRole::Body];
const CAPTION: FaceRole = FaceRole::Caption;
/// Rows from the caption's baseline to the value's capitals.
const UNDER_CAPTION_UNITS: usize = 7;

/// A cell's setting byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Celled {
    pub metric: Metric,
    pub large: bool,
    pub centre: bool,
}

const _: () = assert!(
    Metric::ALL.len() <= 64,
    "a cell keeps six bits for its metric"
);

impl Celled {
    pub const fn byte(self) -> u8 {
        self.metric.id() | (self.large as u8) << 6 | (self.centre as u8) << 7
    }

    fn looks(self) -> &'static [FaceRole] {
        &LADDER[usize::from(!self.large)..]
    }
}

impl Arg for Celled {
    fn decode(raw: u8) -> Option<Self> {
        Some(Self {
            metric: Metric::from_id(raw & 0x3f)?,
            large: raw >> 6 & 1 == 1,
            centre: raw >> 7 & 1 == 1,
        })
    }

    fn encode(self) -> u8 {
        self.byte()
    }

    fn label(self) -> &'static str {
        self.metric.name()
    }
}

fn caption(metric: Metric, out: &mut Text<12>) {
    let _ = out.push_str(match metric {
        Metric::Distance => "DIST KM",
        Metric::NormalizedPower => "NP W",
        Metric::Ascent => "ASCENT M",
        Metric::MaxSpeed => "MAX KM/H",
        metric => metric.caption(),
    });
}

/// The longest a cell's ride time reads in hours and minutes.
const LAST_MINUTE: i32 = 99 * 3600 + 59 * 60;

fn write(metric: Metric, value: Option<i32>, out: &mut Text<12>) {
    let bounded = |low: i32, high: i32| value.map(|value| value.clamp(low, high));
    match metric {
        Metric::Distance => metric.write(bounded(0, 999_900), out),
        Metric::Ascent => metric.write(bounded(0, 9_999), out),
        Metric::Moving => match bounded(0, LAST_MINUTE) {
            Some(seconds) if seconds >= 3600 => {
                let _ = write!(out, "{}:{:02}", seconds / 3600, seconds / 60 % 60);
            }
            seconds => metric.write(seconds, out),
        },
        metric => metric.write(value, out),
    }
}

/// Digits share one advance, so the widest text is at an edge: no value,
/// either end, -1, the last second before H:MM, the last count before
/// thousands.
fn widest(metric: Metric, face: &Face) -> usize {
    [
        None,
        Some(i32::MIN),
        Some(-1),
        Some(3_599),
        Some(9_999),
        Some(i32::MAX),
    ]
    .into_iter()
    .map(|value| {
        let mut text = Text::<12>::new();
        write(metric, value, &mut text);
        face.fit_width(text.as_bytes())
    })
    .max()
    .unwrap_or_default()
}

impl Tile for RideCell {
    type Arg = Celled;
    const NAME: &'static str = "ride cell";

    fn reads(celled: Celled) -> Option<Reads> {
        Some(Reads::Metric(celled.metric))
    }

    fn forms(celled: Celled, at: &Scaled) -> Forms {
        let metric = celled.metric;
        let face = at.face(CAPTION);
        let mut text = Text::<12>::new();
        caption(metric, &mut text);
        let named = face.fit_width(text.as_bytes()) as u16;
        let over = face.fit_height() as u16 + at.unit(UNDER_CAPTION_UNITS) as u16;
        celled
            .looks()
            .iter()
            .map(|&role| {
                let face = at.face(role);
                Form {
                    width: (widest(metric, face) as u16).max(named),
                    height: over + u16::from(face.cap),
                    shapes: 0,
                    runs: 2,
                }
            })
            .collect()
    }

    fn prepare(celled: Celled, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let metric = celled.metric;
        let rect = placed.rect;
        let (left, right) = (usize::from(rect.left), usize::from(rect.right));
        let (x, align) = if celled.centre {
            ((left + right - SPILL) / 2, Align::Center)
        } else {
            (left, Align::Left)
        };
        let mut named = Text::<12>::new();
        caption(metric, &mut named);
        let face = at.face(CAPTION);
        let base = usize::from(rect.top) + face.fit_height();
        out.caption(at, named.as_bytes(), right - left, x, base, align);
        let value = data.metric(metric);
        let mut text = Text::<12>::new();
        write(metric, value, &mut text);
        let role = celled.looks()[usize::from(placed.form)];
        let base = base + at.unit(UNDER_CAPTION_UNITS) + usize::from(at.face(role).cap);
        out.fit(
            &[at.face(role)],
            text.as_bytes(),
            right - left,
            x,
            base,
            align,
            metric.color(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::tile::Tile;
    use crate::screens::panels::ALL;

    #[test]
    fn every_cell_value_fits_its_smallest_look() {
        let mut values = std::vec![None, Some(i32::MIN), Some(i32::MAX), Some(0), Some(-1)];
        let mut seed = 7u64;
        for _ in 0..400 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            values.push(Some((seed >> 33) as i32 - (1 << 29)));
            values.push(Some((seed >> 45) as i32 - 2_000));
        }
        for device in ALL {
            let at = &device.scaled;
            for metric in Metric::ALL {
                for large in [false, true] {
                    let celled = Celled {
                        metric,
                        large,
                        centre: false,
                    };
                    let forms = RideCell::forms(celled, at);
                    let face = at.face(*celled.looks().last().unwrap());
                    let room = usize::from(forms[forms.len() - 1].width);
                    for &value in &values {
                        let mut text = Text::<12>::new();
                        write(metric, value, &mut text);
                        assert!(
                            face.fit_width(text.as_bytes()) <= room,
                            "{metric:?} {text:?} on {:?}",
                            at.panel
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_ride_time_past_an_hour_reads_in_hours_and_minutes() {
        let text = |metric, seconds| {
            let mut text = Text::<12>::new();
            write(metric, Some(seconds), &mut text);
            std::string::String::from(text.as_str())
        };
        assert_eq!(text(Metric::Moving, 323), "05:23");
        assert_eq!(text(Metric::Moving, 3599), "59:59");
        assert_eq!(text(Metric::Moving, 3600 + 23 * 60 + 59), "1:23");
        assert_eq!(text(Metric::Distance, 99_994), "99.99");
        assert_eq!(text(Metric::Distance, i32::MAX), "999.9");
        assert_eq!(text(Metric::Ascent, 12_000), "9999");
    }
}
