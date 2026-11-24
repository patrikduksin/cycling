use crate::compose::data::Data;
use crate::compose::input::{Action, SheetAct};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, BLACK, RAISED, SECONDARY, WHITE};
use crate::screens::icons::{Sensor, Size, StaticIcon};
use crate::screens::scaled::Scaled;
use crate::screens::sheet::SENSORS;
use firmware_shell::rendering::font::FaceRole;

const RADIUS_UNITS: usize = 14;
const PAD_UNITS: usize = 12;

fn disc(out: &mut Canvas, (cx, cy): (usize, usize), radius: usize, color: u16) {
    out.shape(Shape::Disc {
        cx: 4 * cx as i32,
        cy: 4 * cy as i32,
        radius: 4 * radius as i32,
        color,
    });
}

pub struct Level;

impl Tile for Level {
    type Arg = ();
    const NAME: &'static str = "level";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(120) as u16,
            height: at.unit(36) as u16,
            shapes: 3,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(facts) = data.sheet() else {
            return;
        };
        let rect = placed.rect.rect();
        let split = rect.left + rect.width() * usize::from(facts.level.min(100)) / 100;
        out.shape(Shape::Split {
            rect,
            radius: at.unit(RADIUS_UNITS),
            split,
            colors: [WHITE, RAISED],
        });
        let middle = rect.top + rect.height() / 2;
        let pad = at.unit(PAD_UNITS);
        disc(
            out,
            (rect.left + pad + at.unit(5), middle),
            at.unit(4),
            BLACK,
        );
        disc(
            out,
            (rect.right - pad - at.unit(7), middle),
            at.unit(6),
            SECONDARY,
        );
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Sheet(SheetAct::Level))
    }
}

pub struct Sensors;

impl Tile for Sensors {
    type Arg = ();
    const NAME: &'static str = "sheet sensors";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(130) as u16,
            height: at.unit(40) as u16,
            shapes: 1 + 2 * SENSORS as u8,
            runs: 2,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(facts) = data.sheet() else {
            return;
        };
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: at.unit(RADIUS_UNITS),
            color: RAISED,
        });
        let (pad, pitch) = (at.unit(PAD_UNITS), at.unit(28));
        let caption = at.face(FaceRole::Caption);
        let (gap, icons) = (at.unit(4), at.art.sensor(Sensor::Heart, Size::Small).size);
        let block = usize::from(icons) + gap + caption.fit_height();
        let top = rect.top + rect.height().saturating_sub(block) / 2;
        let mut x = rect.left + pad;
        for glance in &facts.sensors {
            let sensor = Sensor::of(glance.kind);
            let icon = at.art.sensor(sensor, Size::Small);
            out.shape(Shape::Icon {
                icon: StaticIcon(icon),
                left: x,
                top,
                color: sensor.tint(),
            });
            let side = usize::from(icon.size);
            disc(out, (x + side, top + at.unit(2)), at.unit(3), glance.color);
            x += pitch;
        }
        let chevron = rect.right - pad;
        let width = chevron.saturating_sub(rect.left + 2 * pad);
        let base = top + block;
        out.fit(
            &[caption],
            b"SENSORS",
            width,
            rect.left + pad,
            base,
            Align::Left,
            WHITE,
        );
        let body = at.face(FaceRole::Body);
        let middle = rect.top + (rect.height() + body.fit_height()) / 2;
        out.fit(&[body], b">", pad, chevron, middle, Align::Right, SECONDARY);
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Sheet(SheetAct::Sensors))
    }
}

pub struct DayNight;

impl Tile for DayNight {
    type Arg = ();
    const NAME: &'static str = "day night";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(40) as u16,
            height: at.unit(40) as u16,
            shapes: 3,
            runs: 1,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(facts) = data.sheet() else {
            return;
        };
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: at.unit(RADIUS_UNITS),
            color: RAISED,
        });
        let caption = at.face(FaceRole::Caption);
        let gap = at.unit(5);
        let (radius, centre) = (at.unit(7), rect.center_x());
        let block = 2 * radius + gap + caption.fit_height();
        let top = rect.top + rect.height().saturating_sub(block) / 2;
        let cy = top + radius;
        disc(out, (centre, cy), radius, WHITE);
        if !facts.light {
            disc(out, (centre + radius / 2, cy - radius / 2), radius, RAISED);
        }
        let word: &[u8] = if facts.light { b"DAY" } else { b"NIGHT" };
        let base = top + block;
        out.fit(
            &[caption],
            word,
            rect.width(),
            centre,
            base,
            Align::Center,
            SECONDARY,
        );
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Sheet(SheetAct::DayNight))
    }
}
