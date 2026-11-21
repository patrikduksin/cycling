//! The workout step's target on a gauge: the track from 70 % of the band's
//! low end to 130 % of its high end, the band on it at 73 % of the metric
//! colour so it reads in sunlight, and the reading as a white disc.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{BLACK, Rect, TERTIARY, WHITE, mix};
use crate::screens::ride::workout::metric_color;
use crate::screens::scaled::Scaled;

pub struct TargetBand;

const TRACK_UNITS: usize = 8;
const DISC_UNITS: usize = 5;

impl Tile for TargetBand {
    type Arg = ();
    const NAME: &'static str = "target band";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(8 * DISC_UNITS) as u16,
            height: at.unit(2 * DISC_UNITS) as u16,
            shapes: 3,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(training) = data.training().filter(|training| !training.done) else {
            return;
        };
        let now = &training.now;
        let Some(band) = now.band else {
            return;
        };
        let rect = placed.rect.rect();
        let middle = (rect.top + rect.bottom) / 2;
        let track = Rect {
            top: middle - at.unit(TRACK_UNITS) / 2,
            bottom: middle - at.unit(TRACK_UNITS) / 2 + at.unit(TRACK_UNITS),
            ..rect
        };
        let low = u32::from(band.low) * 7 / 10;
        let high = (u32::from(band.high) * 13 / 10).max(low + 1);
        let x = |value: u32| {
            track.left
                + (value.clamp(low, high) - low) as usize * track.width() / (high - low) as usize
        };
        let radius = at.unit(4);
        out.shape(Shape::Bar {
            rect: track,
            radius,
            color: TERTIARY,
        });
        let (from, to) = (
            x(band.low.into()),
            x(band.high.into()).max(x(band.low.into()) + at.unit(6)),
        );
        out.shape(Shape::Bar {
            rect: Rect {
                left: from,
                right: to.min(track.right),
                ..track
            },
            radius,
            color: mix(metric_color(band.metric), BLACK, 11),
        });
        if let Some(reading) = now.reading {
            let disc = at.unit(DISC_UNITS);
            let cx = x(reading.into()).clamp(track.left + disc, track.right - disc);
            out.shape(Shape::Disc {
                cx: 4 * cx as i32,
                cy: 4 * (track.top + track.height() / 2) as i32,
                radius: 4 * disc as i32,
                color: WHITE,
            });
        }
    }
}
