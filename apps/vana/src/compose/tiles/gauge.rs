use crate::compose::data::{Data, Effort};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{RAISED, Rect};
use crate::screens::scaled::Scaled;

pub struct Gauge;

struct Look {
    segment_width_units: usize,
    height_units: usize,
}

const LOOKS: [Look; 2] = [
    Look {
        segment_width_units: 24,
        height_units: 6,
    },
    Look {
        segment_width_units: 12,
        height_units: 4,
    },
];
const GAP_UNITS: usize = 2;

impl Tile for Gauge {
    type Arg = Effort;
    const NAME: &'static str = "gauge";

    fn forms(effort: Effort, at: &Scaled) -> Forms {
        let count = usize::from(effort.zones());
        LOOKS
            .iter()
            .map(|look| Form {
                width: (count * at.unit(look.segment_width_units)
                    + (count - 1) * at.unit(GAP_UNITS)) as u16,
                height: at.unit(look.height_units) as u16,
                shapes: count as u8,
                runs: 0,
            })
            .collect()
    }

    fn prepare(effort: Effort, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let look = &LOOKS[usize::from(placed.form)];
        let (width, height) = (
            at.unit(look.segment_width_units),
            at.unit(look.height_units),
        );
        let gap = at.unit(GAP_UNITS);
        let lit = data.zone(effort).unwrap_or(0);
        let rect = placed.rect.rect();
        for segment in 0..effort.zones() {
            let left = rect.left + usize::from(segment) * (width + gap);
            out.shape(Shape::Bar {
                rect: Rect {
                    left,
                    top: rect.bottom - height,
                    right: left + width,
                    bottom: rect.bottom,
                },
                radius: at.unit(1),
                color: if segment < lit {
                    effort.color()
                } else {
                    RAISED
                },
            });
        }
    }
}
