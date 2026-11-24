//! A horizontal bar: an effort's zones, each lit segment in its own zone's
//! colour up to the zone it sits in, or a level, lit from the left up to a
//! metric's share of its full scale.
use super::{px, specs};
use crate::compose::data::{Data, Effort, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Rect;
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Tone};

pub struct Bar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    /// One segment a zone.
    Zones(Effort),
    /// The metric's reading out of `full`, lit in `tone`.
    Level { metric: Metric, full: i32, tone: Tone },
}

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub shows: Shows,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }

    const fn shapes(&self) -> u8 {
        match self.shows {
            Shows::Zones(effort) => effort.zones(),
            Shows::Level { .. } => 1,
        }
    }
}

specs! {
    /// A bar. Ids: Stats and Climb 0–63, Workout 64–127, home and sheet
    /// 128–191.
    pub enum Gauge: Spec {
        PowerZones = 0 => Spec { name: "POWER ZONES", shows: Shows::Zones(Effort::Power) },
        HeartZones = 1 => Spec { name: "HEART ZONES", shows: Shows::Zones(Effort::Heart) },
    }
}

/// Columns between two zone segments.
const SEAM_UNITS: u8 = 2;

impl Tile for Bar {
    type Arg = Gauge;
    const NAME: &'static str = "bar";

    fn reads(gauge: Gauge) -> Option<Reads> {
        match gauge.spec().shows {
            Shows::Zones(Effort::Power) => Some(Reads::Metric(Metric::Power)),
            Shows::Zones(Effort::Heart) => Some(Reads::Metric(Metric::Heart)),
            Shows::Level { metric, .. } => Some(Reads::Metric(metric)),
        }
    }

    fn forms(gauge: Gauge, at: &Scaled) -> Forms {
        let spec = gauge.spec();
        let segments = usize::from(spec.shapes());
        forms(&[Form {
            width: (segments * (px(at, SEAM_UNITS) + 1)) as u16,
            height: px(at, LAYOUT.space.bar) as u16,
            shapes: spec.shapes(),
            runs: 0,
        }])
    }

    fn prepare(gauge: Gauge, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let theme = data.theme();
        let rect = placed.rect.rect();
        let radius = px(at, LAYOUT.radius.bar);
        match gauge.spec().shows {
            Shows::Zones(effort) => {
                let lit = data.zone(effort).unwrap_or(0);
                let count = usize::from(effort.zones());
                let seam = px(at, SEAM_UNITS);
                let pitch = (rect.width() + seam) / count;
                for zone in 0..count {
                    let left = rect.left + zone * pitch;
                    let right = if zone + 1 == count { rect.right } else { left + pitch - seam };
                    let color = if zone < usize::from(lit) {
                        theme.zone(zone as u8 + 1)
                    } else {
                        theme.off
                    };
                    out.shape(Shape::Bar {
                        rect: Rect { left, right, ..rect },
                        radius,
                        color,
                    });
                }
            }
            Shows::Level { metric, full, tone } => {
                let reading = data.metric(metric).unwrap_or(0).clamp(0, full);
                let split = rect.left + rect.width() * reading as usize / full.max(1) as usize;
                out.shape(Shape::Split {
                    rect,
                    radius,
                    split,
                    colors: [theme.tone(tone), theme.off],
                });
            }
        }
    }
}
