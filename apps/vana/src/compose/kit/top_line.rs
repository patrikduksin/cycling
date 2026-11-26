//! A ride page's top line: the recording dot and the ride's time on the
//! left, the satellite in the colour of the fix and the battery glyph in
//! the colour of its level on the right.
use super::px;
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ATTENTION_BATTERY, Align, Battery, Gps, LOW_BATTERY, Rect};
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::Text;
use core::fmt::Write;

pub struct TopLine;

pub(crate) const fn gps_color(theme: &Theme, gps: Gps) -> u16 {
    match gps {
        Gps::Fixed => theme.good,
        Gps::Searching => theme.warn,
        Gps::Off => theme.off,
    }
}

pub(crate) const fn battery_color(theme: &Theme, battery: Battery) -> u16 {
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

const BODY: (usize, usize) = (17, 10);
const NUB: (usize, usize) = (2, 4);
const BODY_RADIUS: usize = 3;
const FILL_INSET: usize = 2;

pub(crate) const fn battery_glyph_width(at: &Scaled) -> usize {
    at.unit(BODY.0 + NUB.0)
}

pub(crate) fn battery_glyph(
    out: &mut Canvas,
    at: &Scaled,
    theme: &Theme,
    (right, middle): (usize, usize),
    (fill, color): (u8, u16),
    under: u16,
) -> usize {
    let (width, height) = (at.unit(BODY.0), at.unit(BODY.1));
    let (nub_width, nub_height) = (at.unit(NUB.0), at.unit(NUB.1));
    let left = right.saturating_sub(width + nub_width);
    let top = middle.saturating_sub(height / 2);
    let body = Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    };
    let radius = at.unit(BODY_RADIUS);
    out.shape(Shape::Bar {
        rect: body,
        radius,
        color: theme.sub,
    });
    out.shape(Shape::Bar {
        rect: body.inset(1),
        radius: radius - 1,
        color: under,
    });
    let nub_top = middle.saturating_sub(nub_height / 2);
    out.shape(Shape::Bar {
        rect: Rect {
            left: body.right,
            top: nub_top,
            right: body.right + nub_width,
            bottom: nub_top + nub_height,
        },
        radius: 0,
        color: theme.sub,
    });
    let inside = body.inset(FILL_INSET);
    let columns = (usize::from(fill) * inside.width()).div_ceil(100);
    if columns > 0 {
        out.shape(Shape::Bar {
            rect: Rect {
                right: inside.left + columns,
                ..inside
            },
            radius: 1,
            color,
        });
    }
    left
}

pub(crate) fn battery(
    out: &mut Canvas,
    at: &Scaled,
    theme: &Theme,
    battery: Battery,
    (right, middle): (usize, usize),
    under: u16,
) -> usize {
    let color = battery_color(theme, battery);
    let fill = (battery.fill(), color);
    let left = battery_glyph(out, at, theme, (right, middle), fill, under);
    if battery.charging || battery.percent >= LOW_BATTERY {
        return left;
    }
    let tag = at.face(LAYOUT.face.tag);
    let mut percent = Text::<5>::new();
    let _ = write!(percent, "{}%", battery.percent);
    let x = left.saturating_sub(px(at, LAYOUT.space.tight));
    let base = middle + usize::from(tag.cap) / 2;
    let run = out.fit(&[tag], percent.as_bytes(), x, x, base, Align::Right, color);
    run.left
}

pub(crate) fn battery_width(at: &Scaled) -> usize {
    let percent = at.face(LAYOUT.face.tag).fit_width(b"19%");
    battery_glyph_width(at) + px(at, LAYOUT.space.tight) + percent
}

pub(crate) const BATTERY_SHAPES: u8 = 4;

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
        let battery = battery_width(at);
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
            shapes: 2 + BATTERY_SHAPES,
            runs: 3,
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
            color: if data.recording() {
                theme.bad
            } else {
                theme.warn
            },
        });
        let line = at.face(LAYOUT.face.line);
        let mut time = Text::<12>::new();
        Metric::Elapsed.write(data.metric(Metric::Elapsed), &mut time);
        let base = middle + line.cap as usize / 2;
        let at_time = left + dot + px(at, space.gap);
        out.fit(
            &[line],
            time.as_bytes(),
            right - at_time,
            at_time,
            base,
            Align::Left,
            theme.text,
        );
        let status = data.status();
        let mut x = right;
        if let Some(level) = status.battery {
            let left = battery(out, at, theme, level, (right, middle), theme.bg);
            x = left.saturating_sub(px(at, space.tight));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::CHARGE_STEPS;
    use crate::screens::scaled::BASE;
    use crate::screens::theme::DARK;

    fn drawn(level: Battery) -> (Canvas, usize) {
        let mut out = Canvas::default();
        let left = battery(&mut out, &BASE, &DARK, level, (200, 12), DARK.bg);
        (out, left)
    }

    fn fill(out: &Canvas) -> Option<(usize, u16)> {
        out.shapes.get(3).map(|shape| match *shape {
            Shape::Bar { rect, color, .. } => (rect.width(), color),
            _ => panic!("{shape:?}"),
        })
    }

    #[test]
    fn the_glyph_fills_with_the_level_in_its_colour_and_a_low_one_says_its_percentage() {
        let (out, left) = drawn(Battery::level(80));
        assert!(out.runs.is_empty(), "the glyph alone");
        assert_eq!(left, 200 - battery_glyph_width(&BASE));
        assert_eq!(fill(&out), Some((11, DARK.text)), "80% of 13 columns");
        let (out, _) = drawn(Battery::level(25));
        assert_eq!(fill(&out).map(|(_, color)| color), Some(DARK.warn));
        let (out, left) = drawn(Battery::level(12));
        let glyph = 200 - battery_glyph_width(&BASE);
        assert_eq!(fill(&out).map(|(_, color)| color), Some(DARK.bad));
        assert_eq!(out.find("12%").map(|run| run.color), Some(DARK.bad));
        assert!(
            left < glyph && out.runs[0].right() < glyph,
            "left of the glyph"
        );
        let charging = Battery {
            charging: true,
            phase: CHARGE_STEPS - 1,
            ..Battery::level(12)
        };
        let (out, _) = drawn(charging);
        assert!(out.runs.is_empty(), "charging keeps the glyph alone");
        assert_eq!(fill(&out), Some((13, DARK.good)), "the fill climbs to full");
    }
}
