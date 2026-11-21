//! The road's profile, which the climb page's chart painter fills row by
//! row in this tile's box, with the rider's disc on it and, on the map's
//! climb, the progress bar under it from FOOT to TOP.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect, TERTIARY, WHITE};
use crate::screens::ride::climb::{
    MIN_CHART_UNITS, RIDER_RADIUS, barred, barred_units, chart_width,
};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct ClimbProfile;

impl Tile for ClimbProfile {
    type Arg = ();
    const NAME: &'static str = "climb profile";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: chart_width(at) as u16,
            height: (at.unit(MIN_CHART_UNITS) + barred_units(at)) as u16,
            shapes: 3,
            runs: 3,
        }])
    }

    fn heavy() -> bool {
        true
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(road) = data.road() else {
            return;
        };
        let frame = road.frame;
        if let Some(done) = frame.done {
            let chart = placed.rect.rect();
            let under = barred(chart, at);
            let (track, radius) = (under.bar, at.unit(3));
            out.shape(Shape::Bar {
                rect: track,
                radius,
                color: TERTIARY,
            });
            let filled = chart.width() * usize::from(done) / 1000;
            if filled >= track.height() {
                out.shape(Shape::Bar {
                    rect: Rect {
                        right: chart.left + filled,
                        ..track
                    },
                    radius,
                    color: WHITE,
                });
            }
            let width = chart.width() / 3;
            let base = under.labels_base;
            out.caption(at, b"FOOT", width, chart.left, base, Align::Left);
            let mut share = Text::<12>::new();
            let _ = write!(share, "{}% UP", (done + 5) / 10);
            out.fit(
                &[at.face(FaceRole::Caption)],
                share.as_bytes(),
                width,
                (chart.left + chart.right) / 2,
                base,
                Align::Center,
                WHITE,
            );
            out.caption(at, b"TOP", width, chart.right, base, Align::Right);
        }
        if let Some((cx, cy)) = frame.rider() {
            out.shape(Shape::Disc {
                cx,
                cy,
                radius: at.unit(RIDER_RADIUS) as i32,
                color: WHITE,
            });
        }
    }
}
