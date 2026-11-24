use crate::compose::data::{Data, PageData};
use crate::compose::input::{Action, Go, Plan};
use crate::compose::kit::top_line::{battery_color, gps_color};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Gps, Rect, mix};
use crate::screens::home::{self, Home};
use crate::screens::icons::{Icon, Sensor, Size, StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::Glance;
use crate::screens::theme::{LAYOUT, Theme, Tone};
use crate::screens::workout::{Page, Text};
use crate::sensors::ant::{BatteryStatus, device_type};
use core::fmt::Write;

const RING_GAP: usize = 2;
const CARD_RADIUS: usize = 18;
const CARD_PAD: usize = 12;
const DISC_GAP: usize = 10;

pub const FACE_UNITS: u8 = 100;
const DATE_BASE: usize = 39;
const TIME_BASE: usize = 96;
const BATTERY_LINE: usize = 24;
const FACE_INSET: usize = 14;
const BATTERY_INSET: usize = 12;

pub const WIDGET_UNITS: u8 = 26;
const WIDGET_PAD: usize = 10;
const WIDGET_GAP: usize = 8;
const DIVIDER: (usize, usize) = (14, 3);
const DOT: usize = 6;

pub const TODAY_UNITS: u8 = 88;
const TITLE_ROW: usize = 36;
const STRIP_GAP: usize = 9;
const STRIP_BOX: usize = 26;
const STRIP_RADIUS: usize = 7;
const STRIP_INSET: (usize, usize) = (5, 3);
const TODAY_DISC: usize = 30;
/// The zone the card is filled with: sweet spot, the colour of training.
const TODAY_ZONE: u8 = 4;
const SUB_INK: u8 = 11;

fn disc(out: &mut Canvas, (cx, cy): (usize, usize), diameter: usize, color: u16) {
    out.shape(Shape::Disc {
        cx: (4 * cx) as i32,
        cy: (4 * cy) as i32,
        radius: (2 * diameter) as i32,
        color,
    });
}

fn play(out: &mut Canvas, (cx, cy): (usize, usize), diameter: usize, color: u16) {
    let (width, height) = (diameter * 3 / 10, diameter * 7 / 20);
    let left = cx + diameter / 20 - width / 2;
    out.shape(Shape::Play {
        rect: Rect {
            left,
            top: cy - height / 2,
            right: left + width,
            bottom: cy - height / 2 + height,
        },
        color,
    });
}

fn ring(out: &mut Canvas, card: Rect, radius: usize, at: &Scaled, theme: &Theme) {
    let (ring, gap) = (at.unit(usize::from(LAYOUT.space.ring)), at.unit(RING_GAP));
    let around = |by: usize| Rect {
        left: card.left.saturating_sub(by),
        top: card.top.saturating_sub(by),
        right: card.right + by,
        bottom: card.bottom + by,
    };
    out.shape(Shape::Panel {
        rect: around(ring + gap),
        radius: radius + ring + gap,
        color: theme.focus,
    });
    out.shape(Shape::Panel {
        rect: around(gap),
        radius: radius + gap,
        color: theme.bg,
    });
}

fn home(data: &dyn Data) -> Option<&Home> {
    match data.page() {
        PageData::Home(home) => Some(home),
        _ => None,
    }
}

/// The battery at the top right, the date and the time.
pub struct Face;

impl Tile for Face {
    type Arg = ();
    const NAME: &'static str = "face";

    fn forms((): (), at: &Scaled) -> Forms {
        let date = at.face(LAYOUT.face.note).fit_width(b"Wednesday 30 Sep");
        let time = at.face(LAYOUT.face.clock).fit_width(b"12:59");
        forms(&[Form {
            width: (2 * at.unit(FACE_INSET) + date.max(time)) as u16,
            height: at.unit(usize::from(FACE_UNITS)) as u16,
            shapes: 0,
            runs: 3,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(home) = home(data) else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        let (left, width) = (rect.left + at.unit(FACE_INSET), rect.width());
        if let Some(battery) = data.status().battery {
            let tag = at.face(LAYOUT.face.tag);
            let mut percent = Text::<5>::new();
            let _ = write!(percent, "{}%", battery.percent.min(100));
            let base = rect.top + (at.unit(BATTERY_LINE) + usize::from(tag.cap)) / 2;
            let right = rect.right - at.unit(BATTERY_INSET);
            let color = battery_color(theme, battery);
            out.fit(
                &[tag],
                percent.as_bytes(),
                width,
                right,
                base,
                Align::Right,
                color,
            );
        }
        if let Some(local) = home.local {
            let date = home::date(local);
            let base = rect.top + at.unit(DATE_BASE);
            let note = at.face(LAYOUT.face.note);
            out.fit(
                &[note],
                date.as_bytes(),
                width,
                left,
                base,
                Align::Left,
                theme.sub,
            );
        }
        let time = home::time(home.local);
        let base = rect.top + at.unit(TIME_BASE);
        let clock = at.face(LAYOUT.face.clock);
        out.fit(
            &[clock],
            time.as_bytes(),
            width,
            left,
            base,
            Align::Left,
            theme.text,
        );
    }
}

/// The fix, then up to three sensors, each with a dot: green live, amber
/// not yet. ACT or a tap opens the sensors.
pub struct Widget;

/// A sensor's icon and dot colours: a live one in its metric's colour with
/// a green dot, amber for a low battery and red for a critical one; one not
/// live yet grey with an amber dot.
pub(super) fn sensor_colors(theme: &Theme, glance: &Glance) -> (u16, u16) {
    if !glance.live {
        return (theme.sub, theme.warn);
    }
    let dot = match glance.battery {
        Some(BatteryStatus::Critical) => theme.bad,
        Some(status) if status.is_low() => theme.warn,
        _ => theme.good,
    };
    (theme.tone(sensor_tone(glance.kind)), dot)
}

pub(super) const fn sensor_tone(kind: u8) -> Tone {
    match kind {
        device_type::HEART_RATE => Tone::Heart,
        device_type::POWER => Tone::Power,
        device_type::SPEED | device_type::SPEED_CADENCE => Tone::Speed,
        device_type::CADENCE => Tone::Cadence,
        _ => Tone::Text,
    }
}

struct Pill {
    rect: Rect,
    satellite: usize,
    divider: usize,
    sensors: heapless::Vec<usize, { home::SENSORS }>,
}

impl Pill {
    fn of(sensors: &[Glance], left: usize, top: usize, at: &Scaled) -> Self {
        let (pad, gap) = (at.unit(WIDGET_PAD), at.unit(WIDGET_GAP));
        let satellite = left + pad;
        let after = satellite + at.art.symbol(Symbol::SatelliteWide).ink(0, 0).width();
        let mut x = after + gap;
        let divider = x + at.unit(DIVIDER.1);
        x = divider + 1 + at.unit(DIVIDER.1) + gap;
        let mut at_x = heapless::Vec::new();
        for glance in sensors {
            let _ = at_x.push(x);
            x += sensor_icon(glance.kind, at).ink(0, 0).width() + gap;
        }
        let right = if sensors.is_empty() { after } else { x - gap } + pad;
        Self {
            rect: Rect {
                left,
                top,
                right,
                bottom: top + at.unit(usize::from(WIDGET_UNITS)),
            },
            satellite,
            divider,
            sensors: at_x,
        }
    }
}

fn sensor_icon(kind: u8, at: &Scaled) -> &'static Icon {
    at.art.sensor(Sensor::of(kind), Size::Small)
}

fn marked(
    out: &mut Canvas,
    icon: &'static Icon,
    (left, middle): (usize, usize),
    (color, dot_color): (u16, u16),
    at: &Scaled,
) {
    let ink = icon.ink(0, 0);
    let top = middle - ink.height() / 2;
    out.shape(Shape::Icon {
        icon: StaticIcon(icon),
        left: left - ink.left,
        top: top - ink.top,
        color,
    });
    disc(out, (left + ink.width(), top), at.unit(DOT), dot_color);
}

fn widget(placed: &Placed, at: &Scaled, data: &dyn Data, focused: bool, out: &mut Canvas) {
    let Some(home) = home(data) else {
        return;
    };
    let theme = data.theme();
    let rect = placed.rect.rect();
    let pill = Pill::of(&home.sensors, rect.left, rect.top, at);
    let radius = pill.rect.height() / 2;
    if focused {
        ring(out, pill.rect, radius, at, theme);
    }
    out.shape(Shape::Panel {
        rect: pill.rect,
        radius,
        color: theme.card,
    });
    let middle = (pill.rect.top + pill.rect.bottom) / 2;
    let gps = data.status().gps;
    let ink = if gps == Gps::Fixed {
        theme.text
    } else {
        theme.sub
    };
    let satellite = at.art.symbol(Symbol::SatelliteWide);
    let colors = (ink, gps_color(theme, gps));
    marked(out, satellite, (pill.satellite, middle), colors, at);
    if home.sensors.is_empty() {
        return;
    }
    let tall = at.unit(DIVIDER.0);
    out.shape(Shape::Bar {
        rect: Rect {
            left: pill.divider,
            top: middle - tall / 2,
            right: pill.divider + 1,
            bottom: middle - tall / 2 + tall,
        },
        radius: 0,
        color: theme.rule,
    });
    for (glance, &left) in home.sensors.iter().zip(&pill.sensors) {
        let icon = sensor_icon(glance.kind, at);
        marked(out, icon, (left, middle), sensor_colors(theme, glance), at);
    }
}

impl Tile for Widget {
    type Arg = ();
    const NAME: &'static str = "status widget";

    fn forms((): (), at: &Scaled) -> Forms {
        let widest = [Glance::ble(device_type::SPEED_CADENCE, true, None); home::SENSORS];
        let pill = Pill::of(&widest, 0, 0, at);
        forms(&[Form {
            width: pill.rect.width() as u16,
            height: pill.rect.height() as u16,
            shapes: 6 + 2 * home::SENSORS as u8,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        widget(placed, at, data, false, out);
    }

    fn focused((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        widget(placed, at, data, true, out);
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Go(Go::Open(Page::Sensors)))
    }
}

/// Today's workout on a fill of its zone, in black ink. ACT or a tap
/// starts it; Home opens on it.
pub struct TodayCard;

fn title_top(card: Rect, at: &Scaled) -> usize {
    let block = at.unit(TITLE_ROW + STRIP_GAP + STRIP_BOX);
    card.top + card.height().saturating_sub(block) / 2
}

fn strip_box(card: Rect, at: &Scaled) -> Rect {
    let top = title_top(card, at) + at.unit(TITLE_ROW + STRIP_GAP);
    Rect {
        left: card.left + at.unit(CARD_PAD),
        top,
        right: card.right - at.unit(CARD_PAD),
        bottom: top + at.unit(STRIP_BOX),
    }
}

/// Where the strip's bars go on the card at `card`.
pub fn strip(card: Rect, at: &Scaled) -> Rect {
    let outer = strip_box(card, at);
    let (side, end) = (at.unit(STRIP_INSET.0), at.unit(STRIP_INSET.1));
    Rect {
        left: outer.left + side,
        top: outer.top + end,
        right: outer.right - side,
        bottom: outer.bottom - end,
    }
}

fn today(placed: &Placed, at: &Scaled, data: &dyn Data, focused: bool, out: &mut Canvas) {
    let Some(today) = home(data).and_then(|home| home.today) else {
        return;
    };
    let theme = data.theme();
    let card = placed.rect.rect();
    let radius = at.unit(CARD_RADIUS);
    if focused {
        ring(out, card, radius, at, theme);
    }
    let fill = theme.zone(TODAY_ZONE);
    let ink = theme.on_fill;
    out.shape(Shape::Panel {
        rect: card,
        radius,
        color: fill,
    });
    let top = title_top(card, at);
    let middle = top + at.unit(TITLE_ROW) / 2;
    let diameter = at.unit(TODAY_DISC);
    let cx = card.left + at.unit(CARD_PAD) + diameter / 2;
    disc(out, (cx, middle), diameter, ink);
    play(out, (cx, middle), diameter, fill);
    let left = card.left + at.unit(CARD_PAD + TODAY_DISC + DISC_GAP);
    let width = card.right - at.unit(CARD_PAD) - left;
    let (label, note) = (at.face(LAYOUT.face.label), at.face(LAYOUT.face.note));
    let title = middle - at.unit(3);
    out.fit(
        &[label],
        today.name.as_bytes(),
        width,
        left,
        title,
        Align::Left,
        ink,
    );
    let sub = mix(ink, fill, SUB_INK);
    let base = middle + at.unit(3) + usize::from(note.cap);
    let words = out.fit(
        &[note],
        b"Today's workout",
        width,
        left,
        base,
        Align::Left,
        sub,
    );
    if let Some(planned) = home::planned(today.planned_secs) {
        let space = note.width(b" ");
        let dot = words.right() + space + at.unit(1);
        disc(
            out,
            (dot, base - usize::from(note.cap) / 2),
            at.unit(3),
            sub,
        );
        let x = dot + at.unit(2) + space;
        out.fit(
            &[note],
            planned.as_bytes(),
            width,
            x,
            base,
            Align::Left,
            sub,
        );
    }
    out.shape(Shape::Panel {
        rect: strip_box(card, at),
        radius: at.unit(STRIP_RADIUS),
        color: ink,
    });
}

impl Tile for TodayCard {
    type Arg = ();
    const NAME: &'static str = "today";

    fn forms((): (), at: &Scaled) -> Forms {
        let label = at.face(LAYOUT.face.label);
        let name = label.fit_width(crate::workout::model::WIDEST_NAME.as_bytes());
        forms(&[Form {
            width: (at.unit(2 * CARD_PAD + TODAY_DISC + DISC_GAP) + name.min(at.unit(150))) as u16,
            height: at.unit(usize::from(TODAY_UNITS)) as u16,
            shapes: 7,
            runs: 3,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        today(placed, at, data, false, out);
    }

    fn focused((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        today(placed, at, data, true, out);
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Start(Plan::Today))
    }

    fn opens((): ()) -> bool {
        true
    }
}

/// "Just ride" under today's card, or the one big Ride button when
/// nothing is planned, which Home opens on.
pub struct Start;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Begin {
    Ride,
    JustRide,
}

pub const RIDE_UNITS: u8 = 74;
pub const JUST_RIDE_UNITS: u8 = 48;

impl Begin {
    const fn units(self) -> usize {
        match self {
            Self::Ride => RIDE_UNITS as usize,
            Self::JustRide => JUST_RIDE_UNITS as usize,
        }
    }

    const fn disc(self) -> usize {
        match self {
            Self::Ride => 46,
            Self::JustRide => TODAY_DISC,
        }
    }

    const fn words(self) -> &'static [u8] {
        match self {
            Self::Ride => b"Ride",
            Self::JustRide => b"Just ride",
        }
    }

    fn face(self, at: &Scaled) -> &'static firmware_shell::rendering::font::Face {
        at.face(match self {
            Self::Ride => LAYOUT.face.heading,
            Self::JustRide => LAYOUT.face.label,
        })
    }
}

impl Arg for Begin {
    fn decode(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Ride),
            1 => Some(Self::JustRide),
            _ => None,
        }
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ride => "RIDE",
            Self::JustRide => "JUST RIDE",
        }
    }
}

fn start(
    begin: Begin,
    placed: &Placed,
    at: &Scaled,
    data: &dyn Data,
    focused: bool,
    out: &mut Canvas,
) {
    let theme = data.theme();
    let card = placed.rect.rect();
    let radius = at.unit(CARD_RADIUS);
    if focused {
        ring(out, card, radius, at, theme);
    }
    out.shape(Shape::Panel {
        rect: card,
        radius,
        color: theme.card,
    });
    let middle = (card.top + card.bottom) / 2;
    let diameter = at.unit(begin.disc());
    let cx = card.left + at.unit(CARD_PAD) + diameter / 2;
    disc(out, (cx, middle), diameter, theme.good);
    play(out, (cx, middle), diameter, theme.on_tone);
    let face = begin.face(at);
    let left = cx + diameter / 2 + at.unit(DISC_GAP);
    let base = middle + usize::from(face.cap) / 2;
    let width = card.right.saturating_sub(left);
    out.fit(
        &[face],
        begin.words(),
        width,
        left,
        base,
        Align::Left,
        theme.text,
    );
}

impl Tile for Start {
    type Arg = Begin;
    const NAME: &'static str = "start";

    fn forms(begin: Begin, at: &Scaled) -> Forms {
        let words = begin.face(at).fit_width(begin.words());
        forms(&[Form {
            width: (at.unit(2 * CARD_PAD + begin.disc() + DISC_GAP) + words) as u16,
            height: at.unit(begin.units()) as u16,
            shapes: 5,
            runs: 1,
        }])
    }

    fn prepare(begin: Begin, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        start(begin, placed, at, data, false, out);
    }

    fn focused(begin: Begin, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        start(begin, placed, at, data, true, out);
    }

    fn action(_: Begin) -> Option<Action> {
        Some(Action::Start(Plan::Free))
    }

    fn opens(begin: Begin) -> bool {
        begin == Begin::Ride
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::theme::{DARK, LIGHT};

    #[test]
    fn a_live_sensor_s_dot_says_how_its_battery_is() {
        let strap = |live, battery| Glance::ble(device_type::HEART_RATE, live, battery);
        for theme in [&DARK, &LIGHT] {
            let dot = |glance: Glance| sensor_colors(theme, &glance).1;
            assert_eq!(dot(strap(true, None)), theme.good);
            assert_eq!(dot(strap(true, Some(BatteryStatus::Good))), theme.good);
            assert_eq!(dot(strap(true, Some(BatteryStatus::Low))), theme.warn);
            assert_eq!(dot(strap(true, Some(BatteryStatus::Critical))), theme.bad);
            assert_eq!(dot(strap(false, Some(BatteryStatus::Critical))), theme.warn);
        }
    }
}
