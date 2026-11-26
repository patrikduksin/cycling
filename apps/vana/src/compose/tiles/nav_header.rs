//! A page's header, the same on every page that leads back: the back
//! chevron and the page's title on the left, the clock on the right. Trouble
//! SENSORS names takes the title's place in its tone, and the clock's room
//! too. A tap on the header goes back.
use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Align;
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{List, MenuMessage, NAMES, names, widest};
use crate::screens::theme::{LAYOUT, Tone};
use firmware_shell::rendering::font::Face;

pub struct NavHeader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Titled {
    Sensors = 0,
    Nearby = 1,
    Sensor = 2,
}

impl Arg for Titled {
    fn decode(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Sensors),
            1 => Some(Self::Nearby),
            2 => Some(Self::Sensor),
            _ => None,
        }
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Sensors => "SENSORS",
            Self::Nearby => "NEARBY",
            Self::Sensor => "SENSOR",
        }
    }
}

const WIDEST_CLOCK: &[u8] = b"00:00";

pub fn height(at: &Scaled) -> usize {
    at.face(LAYOUT.face.heading).fit_height() + 2 * px(at, LAYOUT.space.gap)
}

fn faces(at: &Scaled) -> [&'static Face; 2] {
    [at.face(LAYOUT.face.heading), at.face(LAYOUT.face.label)]
}

fn title_left(at: &Scaled) -> usize {
    let chevron = at.art.symbol(Symbol::Chevron).ink(0, 0).width();
    px(at, LAYOUT.space.gutter) + chevron + px(at, LAYOUT.space.gap)
}

fn widest(titled: Titled, at: &Scaled) -> usize {
    let [heading, label] = faces(at);
    let clock = px(at, LAYOUT.space.gap) + at.face(LAYOUT.face.line).fit_width(WIDEST_CLOCK);
    match titled {
        Titled::Sensors => {
            let trouble = MenuMessage::ALL
                .into_iter()
                .filter(|message| message.on_sensors())
                .chain(MenuMessage::NOTICES)
                .map(|message| message.short().as_bytes());
            widest::of(label, trouble).max(heading.fit_width(b"Sensors") + clock)
        }
        Titled::Nearby => heading.fit_width(b"Nearby") + clock,
        Titled::Sensor => widest::of(label, NAMES.map(|names| names.long.as_bytes())) + clock,
    }
}

impl Tile for NavHeader {
    type Arg = Titled;
    const NAME: &'static str = "nav header";

    fn forms(titled: Titled, at: &Scaled) -> Forms {
        let width = title_left(at) + widest(titled, at) + px(at, LAYOUT.space.gutter);
        forms(&[Form {
            width: width as u16,
            height: height(at) as u16,
            shapes: 1,
            runs: 2,
        }])
    }

    fn prepare(titled: Titled, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let (title, tone) = match titled {
            Titled::Sensors => menu.title(List::Sensors),
            Titled::Nearby => menu.title(List::Nearby),
            Titled::Sensor => match menu.shown() {
                Some(sensor) => (names(sensor.identity.device_type).long, Tone::Text),
                None => return,
            },
        };
        let theme = data.theme();
        let space = &LAYOUT.space;
        let rect = placed.rect.rect();
        let faces = faces(at);
        let base = rect.top + (rect.height() + usize::from(faces[0].cap)) / 2;
        let middle = base - usize::from(faces[0].cap) / 2;
        let icon = at.art.symbol(Symbol::Chevron);
        let ink = icon.ink(0, 0);
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: (rect.left + px(at, space.gutter)).saturating_sub(ink.left),
            top: middle.saturating_sub(ink.top + ink.height() / 2),
            color: theme.sub,
        });
        let line = at.face(LAYOUT.face.line);
        let right = rect.right - px(at, space.gutter);
        let left = rect.left + title_left(at);
        let mut end = right;
        if tone == Tone::Text {
            let clock = out.fit(
                &[line],
                data.clock().as_bytes(),
                right.saturating_sub(left),
                right,
                middle + usize::from(line.cap) / 2,
                Align::Right,
                theme.text,
            );
            end = clock.left.saturating_sub(px(at, space.gap));
        }
        out.fit(
            &faces,
            title.as_bytes(),
            end.saturating_sub(left),
            left,
            base,
            Align::Left,
            theme.tone(tone),
        );
    }
}
