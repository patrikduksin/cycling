//! The card a sensor's pages picture it on: its large icon in its tint.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{CARD, Rect};
use crate::screens::icons::{Sensor, Size, StaticIcon};
use crate::screens::scaled::Scaled;

pub struct SensorIcon;

const CARD_UNITS: usize = 56;
const RADIUS_UNITS: usize = 18;

impl Tile for SensorIcon {
    type Arg = ();
    const NAME: &'static str = "sensor_icon";

    fn forms((): (), at: &Scaled) -> Forms {
        let side = at.unit(CARD_UNITS) as u16;
        forms(&[Form {
            width: side,
            height: side,
            shapes: 2,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(identity) = data
            .sensors()
            .and_then(|menu| menu.shown())
            .map(|shown| shown.identity)
        else {
            return;
        };
        let rect = placed.rect.rect();
        let side = at.unit(CARD_UNITS);
        let left = rect.left + (rect.width() - side) / 2;
        let top = rect.top + (rect.height() - side) / 2;
        out.shape(Shape::Panel {
            rect: Rect {
                left,
                top,
                right: left + side,
                bottom: top + side,
            },
            radius: at.unit(RADIUS_UNITS),
            color: CARD,
        });
        let sensor = Sensor::of(identity.device_type);
        let icon = at.art.sensor(sensor, Size::Large);
        let inset = (side - usize::from(icon.size)) / 2;
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: left + inset,
            top: top + inset,
            color: sensor.tint(),
        });
    }
}
