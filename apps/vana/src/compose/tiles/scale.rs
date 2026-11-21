//! The map's foot: the scale bar and its label at the left, the distance
//! and its unit at the right, on one baseline. The distance is what is left
//! of the course followed, else the ride's.
use crate::compose::data::{Data, Metric};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect, SECONDARY, WHITE};
use crate::screens::ride::kilometres;
use crate::screens::ride::map::BAR_MAX_UNITS;
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use firmware_shell::rendering::font::{FaceRole, SPILL};

pub struct Scale;

const MARGIN_UNITS: usize = 12;
const BAR_TOP_UNITS: usize = 13;
const BAR_UNITS: usize = 3;
const BASELINE_UNITS: usize = 19;
/// Between the bar and its label.
const GAP_UNITS: usize = 6;
/// The room for "KM" and the gap before it.
const KM_ROOM: usize = 25;

impl Tile for Scale {
    type Arg = ();
    const NAME: &'static str = "scale";

    fn forms((): (), at: &Scaled) -> Forms {
        let width = 2 * MARGIN_UNITS + BAR_MAX_UNITS + KM_ROOM;
        forms(&[Form {
            width: at.unit(width) as u16,
            height: at.unit(BASELINE_UNITS) as u16,
            shapes: 1,
            runs: 3,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(map) = data.map().filter(|map| map.shows()) else {
            return;
        };
        let rect = placed.rect.rect();
        let (left, right) = (
            rect.left + at.unit(MARGIN_UNITS),
            rect.right - at.unit(MARGIN_UNITS),
        );
        let top = rect.top + at.unit(BAR_TOP_UNITS);
        let bar = Rect {
            left,
            top,
            right: left + at.unit(BAR_MAX_UNITS),
            bottom: top + at.unit(BAR_UNITS),
        };
        let (gap, baseline) = (at.unit(GAP_UNITS), rect.top + at.unit(BASELINE_UNITS));
        let distance_right = right - at.unit(KM_ROOM);
        let mut distance = Text::<12>::new();
        let ridden_m = data.metric(Metric::Distance).map_or(0, |m| m.max(0) as u64);
        let mm = 1000 * map.left_m().map_or(ridden_m, u64::from);
        kilometres(&mut distance, mm);
        let figures = at.face(FaceRole::Foot20);
        let across = distance_right - bar.left;
        let distance_left = distance_right - figures.width(distance.as_bytes()).min(across);
        let caption = at.face(FaceRole::Caption);
        let fits = |length, label: &[u8]| {
            let label_right = bar.left + length + gap + caption.width(label) + SPILL;
            length <= bar.width() && label_right + gap <= distance_left
        };
        if let Some((length, label)) = map.scale(fits) {
            out.shape(Shape::Bar {
                rect: Rect {
                    right: bar.left + length,
                    ..bar
                },
                radius: at.unit(1),
                color: SECONDARY,
            });
            let label_left = bar.left + length + gap;
            let width = distance_left - gap - label_left;
            out.caption(at, label, width, label_left, baseline, Align::Left);
        }
        out.fit(
            &[figures],
            distance.as_bytes(),
            across,
            distance_right,
            baseline,
            Align::Right,
            WHITE,
        );
        out.caption(
            at,
            b"KM",
            right - distance_right,
            right,
            baseline,
            Align::Right,
        );
    }
}
