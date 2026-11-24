//! A ride page's top line: the recording dot and the ride's time on the
//! left, the satellite in the colour of the fix and the battery's
//! percentage in the colour of its level on the right.
use super::px;
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ATTENTION_BATTERY, Align, Battery, Gps, LOW_BATTERY};
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::Text;
use core::fmt::Write;

pub struct TopLine;

const fn gps_color(theme: &Theme, gps: Gps) -> u16 {
    match gps {
        Gps::Fixed => theme.good,
        Gps::Searching => theme.warn,
        Gps::Off => theme.off,
    }
}

const fn battery_color(theme: &Theme, battery: Battery) -> u16 {
    if battery.charging {
        theme.good
    } else if battery.percent < LOW_BATTERY {
        theme.bad
    } else if battery.percent <= ATTENTION_BATTERY {
        theme.warn
    } else {
        theme.text
    }
}

impl Tile for TopLine {
    type Arg = ();
    const NAME: &'static str = "top line";

    fn reads((): ()) -> Option<Reads> {
        Some(Reads::Metric(Metric::Elapsed))
    }

    fn forms((): (), at: &Scaled) -> Forms {
        let space = &LAYOUT.space;
        let time = Metric::Elapsed
            .extremes()
            .iter()
            .map(|text| at.face(LAYOUT.face.line).fit_width(text.as_bytes()))
            .max()
            .unwrap_or_default();
        let satellite = at.art.symbol(Symbol::Satellite).ink(0, 0).width();
        let battery = at.face(LAYOUT.face.tag).fit_width(b"100%");
        let width = 2 * px(at, space.gutter)
            + px(at, space.dot)
            + px(at, space.gap)
            + time
            + px(at, space.gap)
            + satellite
            + px(at, space.tight)
            + battery;
        forms(&[Form {
            width: width as u16,
            height: px(at, space.top_line) as u16,
            shapes: 2,
            runs: 2,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let theme = data.theme();
        let space = &LAYOUT.space;
        let rect = placed.rect.rect();
        let middle = (rect.top + rect.bottom) / 2;
        let left = rect.left + px(at, space.gutter);
        let right = rect.right - px(at, space.gutter);
        let dot = px(at, space.dot);
        out.shape(Shape::Disc {
            cx: (4 * left + 2 * dot) as i32,
            cy: (4 * middle) as i32,
            radius: (2 * dot) as i32,
            color: if data.recording() { theme.bad } else { theme.warn },
        });
        let line = at.face(LAYOUT.face.line);
        let mut time = Text::<12>::new();
        Metric::Elapsed.write(data.metric(Metric::Elapsed), &mut time);
        let base = middle + line.cap as usize / 2;
        let at_time = left + dot + px(at, space.gap);
        out.fit(&[line], time.as_bytes(), right - at_time, at_time, base, Align::Left, theme.text);
        let status = data.status();
        let tag = at.face(LAYOUT.face.tag);
        let mut x = right;
        if let Some(battery) = status.battery {
            let mut percent = Text::<5>::new();
            let _ = write!(percent, "{}%", battery.percent.min(100));
            let base = middle + tag.cap as usize / 2;
            let color = battery_color(theme, battery);
            let run = out.fit(&[tag], percent.as_bytes(), right - at_time, right, base, Align::Right, color);
            x = run.left.saturating_sub(px(at, space.tight));
        }
        let icon = at.art.symbol(Symbol::Satellite);
        let ink = icon.ink(0, 0);
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: x.saturating_sub(ink.left + ink.width()),
            top: middle.saturating_sub(ink.top + ink.height() / 2),
            color: gps_color(theme, status.gps),
        });
    }
}
