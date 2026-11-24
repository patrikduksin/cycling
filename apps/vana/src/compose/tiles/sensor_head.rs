use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::icons::{Sensor, Size, StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{NAMES, Part, Selected, names, widest};
use crate::screens::theme::LAYOUT;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::SPILL;

pub struct SensorHead;

const UNKNOWN_MAKER: &str = "ANT+ sensor";
const FORGET_NOTE: &str = "It stops reconnecting";

fn title(part: Part, sensor: Selected) -> Text<32> {
    let long = names(sensor.identity.device_type).long;
    let mut text = Text::new();
    match part {
        Part::Detail => {
            let _ = text.push_str(long);
        }
        Part::Forget => {
            let (first, rest) = long.split_at(1);
            let first = char::from(first.as_bytes()[0].to_ascii_lowercase());
            let _ = write!(text, "Forget {first}{rest}?");
        }
    }
    text
}

pub fn maker(sensor: Selected) -> &'static str {
    sensor.product.maker().unwrap_or(UNKNOWN_MAKER)
}

fn baselines(at: &Scaled) -> (usize, usize, usize) {
    let space = &LAYOUT.space;
    let (heading, note) = (at.face(LAYOUT.face.heading), at.face(LAYOUT.face.note));
    let title = px(at, space.badge) + px(at, space.gap) + heading.fit_height();
    let subtitle = title + px(at, space.gap) + note.fit_height();
    let bottom = subtitle + usize::from(note.height - note.top - note.cap);
    (title, subtitle, bottom)
}

impl Tile for SensorHead {
    type Arg = Part;
    const NAME: &'static str = "sensor head";

    fn forms(part: Part, at: &Scaled) -> Forms {
        let (heading, note) = (at.face(LAYOUT.face.heading), at.face(LAYOUT.face.note));
        let title = match part {
            Part::Detail => widest::long_name(heading),
            Part::Forget => {
                let label = at.face(LAYOUT.face.label);
                let longest = NAMES.iter().map(|names| label.width(names.long.as_bytes()));
                label.fit_width(b"Forget ?") + longest.max().unwrap_or(0)
            }
        };
        let subtitle = match part {
            Part::Detail => note.fit_width(UNKNOWN_MAKER.as_bytes()),
            Part::Forget => note.fit_width(FORGET_NOTE.as_bytes()),
        };
        let (_, _, height) = baselines(at);
        forms(&[Form {
            width: (2 * px(at, LAYOUT.space.gutter) + title.max(subtitle)) as u16,
            height: height as u16,
            shapes: 2,
            runs: 2,
        }])
    }

    fn prepare(part: Part, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(sensor) = data.sensors().and_then(|menu| menu.shown()) else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        let centre = rect.left + rect.width().saturating_sub(SPILL) / 2;
        let middle = (rect.left + rect.right) / 2;
        let badge = px(at, LAYOUT.space.badge);
        let tile = Rect {
            left: middle - badge / 2,
            top: rect.top,
            right: middle - badge / 2 + badge,
            bottom: rect.top + badge,
        };
        let (icon, color) = match part {
            Part::Detail => {
                out.shape(Shape::Panel {
                    rect: tile,
                    radius: px(at, LAYOUT.radius.card),
                    color: theme.raised,
                });
                let kind = Sensor::of(sensor.identity.device_type);
                (at.art.sensor(kind, Size::Row), theme.tone(kind.tone()))
            }
            Part::Forget => {
                out.shape(Shape::Disc {
                    cx: (2 * (tile.left + tile.right)) as i32,
                    cy: (2 * (tile.top + tile.bottom)) as i32,
                    radius: (2 * badge) as i32,
                    color: theme.bad,
                });
                (at.art.symbol(Symbol::Trash), theme.on_tone)
            }
        };
        let ink = icon.ink(0, 0);
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: (tile.left + tile.right - ink.width()) / 2 - ink.left,
            top: (tile.top + tile.bottom - ink.height()) / 2 - ink.top,
            color,
        });
        let (heading, note) = (at.face(LAYOUT.face.heading), at.face(LAYOUT.face.note));
        let (title_base, subtitle_base, _) = baselines(at);
        out.fit(
            &[heading, at.face(LAYOUT.face.label)],
            title(part, sensor).as_bytes(),
            rect.width(),
            centre,
            rect.top + title_base,
            Align::Center,
            theme.text,
        );
        let subtitle = match part {
            Part::Detail => maker(sensor),
            Part::Forget => FORGET_NOTE,
        };
        out.fit(
            &[note],
            subtitle.as_bytes(),
            rect.width(),
            centre,
            rect.top + subtitle_base,
            Align::Center,
            theme.sub,
        );
    }
}
