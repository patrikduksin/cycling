//! Home's cards: one big RIDE card with the start button and what is still
//! starting, the sensors at a glance, TRAIN (workouts) and SETTINGS side by
//! side, and the saved rides with the last one's numbers. Each opens its
//! page; the focused card is raised inside an accent ring, its title white.
use crate::compose::data::{Data, PageData};
use crate::compose::input::{Action, Go};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{AMBER, Align, BLACK, CARD, GREEN, MUTED, Rect, SECONDARY, WHITE};
use crate::screens::home::{
    Home, ICONS, NONE_SAVED, UNCOUNTED, last_ride, saved, short_name, this_week,
};
use crate::screens::icons::{Sensor, Size, StaticIcon};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Page;
use crate::workout::model::WIDEST_NAME;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct HomeCard;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    Ride,
    Sensors,
    Train,
    Settings,
    Rides,
}

impl Card {
    const ALL: [Self; 5] = [
        Self::Ride,
        Self::Sensors,
        Self::Train,
        Self::Settings,
        Self::Rides,
    ];

    const fn opens(self) -> Page {
        match self {
            Self::Ride => Page::Ready,
            Self::Sensors => Page::Sensors,
            Self::Train => Page::Workouts,
            Self::Settings => Page::Settings,
            Self::Rides => Page::History,
        }
    }

    const fn radius(self) -> usize {
        match self {
            Self::Rides => 12,
            _ => 18,
        }
    }
}

impl Arg for Card {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ride => "RIDE",
            Self::Sensors => "SENSORS",
            Self::Train => "TRAIN",
            Self::Settings => "SETTINGS",
            Self::Rides => "RIDES",
        }
    }
}

const RING_UNITS: usize = 3;
/// Text keeps this far from a card's sides.
const INSET_UNITS: usize = 16;
/// The start button's diameter and its columns from the card's left.
const PLAY_UNITS: usize = 56;
const TITLE_UNITS: usize = 86;
const STATUS_UNITS: usize = 88;
const RIDE_UNITS: usize = 84;
const SENSORS_UNITS: usize = 78;
const SMALL_UNITS: usize = 40;
const RIDES_UNITS: usize = 32;
/// The sensor icons' pitch, and the rows from the sensors card's top to
/// its title, its icons and their names.
const PITCH_UNITS: usize = 52;
const HEADER_UNITS: usize = 14;
const ICONS_UNITS: usize = 35;
const NAMES_UNITS: usize = 58;

fn draw(
    card: Card,
    placed: &Placed,
    at: &Scaled,
    data: &dyn Data,
    out: &mut Canvas,
    focused: bool,
) {
    let PageData::Home(home) = data.page() else {
        return;
    };
    let rect = placed.rect.rect();
    let corners = (at.unit(card.radius()), at.unit(RING_UNITS));
    super::choice::card(out, rect, corners, focused, Some(CARD));
    let title = if focused { WHITE } else { MUTED };
    let [body, caption] = [FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    let inset = at.unit(INSET_UNITS);
    let (left, right) = (rect.left + inset, rect.right - inset);
    let middle = rect.top + (rect.height() + body.cap as usize) / 2;
    match card {
        Card::Ride => ride(home, rect, at, out, title),
        Card::Sensors => sensors(home, rect, at, out, title),
        Card::Train | Card::Settings => {
            let middle = rect.top + (rect.height() + caption.cap as usize) / 2;
            let label = card.label().as_bytes();
            out.fit(
                &[caption],
                label,
                right - left,
                left,
                middle,
                Align::Left,
                title,
            );
        }
        Card::Rides => {
            let rides = out.fit(
                &[body],
                b"RIDES",
                right - left,
                left,
                middle,
                Align::Left,
                title,
            );
            // The last ride keeps its numbers: they step down a size to fit.
            let room = right.saturating_sub(rides.right() + SPILL + at.unit(8));
            let base = middle + at.unit(3) + caption.fit_height() - body.fit_height();
            out.fit(
                &[caption],
                home.last.as_bytes(),
                room,
                right,
                base,
                Align::Right,
                SECONDARY,
            );
        }
    }
}

/// RIDE right of the green start button, then the GPS line and what is
/// still starting on one or two lines.
fn ride(home: &Home, rect: Rect, at: &Scaled, out: &mut Canvas, title: u16) {
    let [heading, caption] = [FaceRole::Title, FaceRole::Caption].map(|role| at.face(role));
    let middle = rect.top + rect.height() / 2;
    let r = at.unit(PLAY_UNITS) / 2;
    let cx = rect.left + at.unit(INSET_UNITS) + r;
    out.shape(Shape::Disc {
        cx: 4 * cx as i32,
        cy: 4 * middle as i32,
        radius: 4 * r as i32,
        color: GREEN,
    });
    let (left, top) = (cx - at.unit(6), middle - at.unit(12));
    out.shape(Shape::Play {
        rect: Rect {
            left,
            top,
            right: left + at.unit(20),
            bottom: top + at.unit(24),
        },
        color: BLACK,
    });
    let right = rect.right - at.unit(8);
    let x = rect.left + at.unit(TITLE_UNITS);
    out.fit(
        &[heading],
        b"RIDE",
        right - x,
        x,
        middle - at.unit(5),
        Align::Left,
        title,
    );
    let x = rect.left + at.unit(STATUS_UNITS);
    let width = right - x;
    let (gps, color): (&[u8], u16) = if home.gps_fix {
        (b"GPS READY", GREEN)
    } else {
        (b"NO GPS FIX", SECONDARY)
    };
    let pitch = caption.fit_height();
    let mut base = middle + pitch;
    out.fit(&[caption], gps, width, x, base, Align::Left, color);
    for (line, gap) in two_lines(caption, home.status.as_bytes(), width)
        .into_iter()
        .zip([4, 2])
    {
        base += at.unit(gap) + pitch;
        if !line.is_empty() {
            out.fit(
                &[caption],
                line,
                width,
                x,
                base,
                Align::Left,
                home.status_color,
            );
        }
    }
}

/// The widest texts the RIDE card's status shows: a workout's name, and
/// the longest of what is still starting and of the battery warnings.
const STATUSES: [&str; 6] = [
    WIDEST_NAME,
    "SENSORS UNAVAILABLE",
    "CHANGES NOT SAVED",
    "SHIFT CRITICAL BATTERY",
    "LIGHT CRITICAL BATTERY",
    "10 BATTERIES TO CHANGE",
];

/// `text` on one line `width` wide, else on two: broken at the last space
/// that leaves both halves fitting, else after the most characters that fit.
fn two_lines<'a>(face: &Face, text: &'a [u8], width: usize) -> [&'a [u8]; 2] {
    let fits = |line: &[u8]| face.fit_width(line) <= width;
    if fits(text) {
        return [text, b""];
    }
    let at_space = (1..text.len())
        .rev()
        .find(|&i| text[i] == b' ' && fits(&text[..i]) && fits(&text[i + 1..]));
    if let Some(i) = at_space {
        return [&text[..i], &text[i + 1..]];
    }
    let cut = (1..text.len())
        .rev()
        .find(|&i| fits(&text[..i]))
        .unwrap_or(0);
    [&text[..cut], text[cut..].trim_ascii_start()]
}

/// The narrowest column `two_lines` keeps every one of `texts` whole in:
/// the first line holds at least the column less a glyph, so the rest
/// fits when the column is half the widest text and a glyph.
fn status_column(face: &Face, texts: &[&str]) -> usize {
    let glyph = face
        .glyphs
        .iter()
        .map(|glyph| usize::from(glyph.advance))
        .max();
    let widest = texts
        .iter()
        .map(|text| face.fit_width(text.as_bytes()))
        .max();
    (widest.unwrap_or_default() + SPILL + glyph.unwrap_or_default()).div_ceil(2)
}

/// The title and the live count, then up to four sensors, each an icon
/// with its status dot and its short name under it.
fn sensors(home: &Home, rect: Rect, at: &Scaled, out: &mut Canvas, title: u16) {
    let [body, caption] = [FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    let inset = at.unit(INSET_UNITS);
    let (left, right) = (rect.left + inset, rect.right - inset);
    let header = rect.top + at.unit(HEADER_UNITS);
    out.fit(
        &[body],
        b"SENSORS",
        right - left,
        left,
        header + body.fit_height(),
        Align::Left,
        title,
    );
    let (glance, live) = (&home.glance, home.live);
    if home.paired > 0 {
        let mut count = crate::screens::workout::Text::<12>::new();
        let _ =
            core::fmt::Write::write_fmt(&mut count, format_args!("{live}/{} LIVE", home.paired));
        let color = if home.battery_low {
            AMBER
        } else if live > 0 {
            GREEN
        } else {
            SECONDARY
        };
        let base = header + at.unit(3) + caption.fit_height();
        out.fit(
            &[caption],
            count.as_bytes(),
            right - left,
            right,
            base,
            Align::Right,
            color,
        );
    }
    let icons = rect.top + at.unit(ICONS_UNITS);
    if glance.is_empty() {
        let base = icons + at.unit(9) + caption.fit_height();
        out.fit(
            &[caption],
            b"NONE PAIRED",
            right - left,
            left,
            base,
            Align::Left,
            SECONDARY,
        );
        return;
    }
    let names = rect.top + at.unit(NAMES_UNITS) + caption.fit_height();
    for (slot, glance) in glance.iter().take(ICONS).enumerate() {
        let x = left + slot * at.unit(PITCH_UNITS);
        let art = at.art.sensor(Sensor::of(glance.kind), Size::Small);
        out.shape(Shape::Disc {
            cx: (4 * x + at.unit(100)) as i32,
            cy: (4 * icons + at.unit(6)) as i32,
            radius: at.unit(10) as i32,
            color: glance.color,
        });
        out.shape(Shape::Icon {
            icon: StaticIcon(art),
            left: x,
            top: icons,
            color: WHITE,
        });
        let centre = x + usize::from(art.size) / 2;
        let name = short_name(glance.kind);
        out.fit(
            &[caption],
            name,
            at.unit(PITCH_UNITS),
            centre,
            names,
            Align::Center,
            SECONDARY,
        );
    }
}

impl Tile for HomeCard {
    type Arg = Card;
    const NAME: &'static str = "home card";

    fn forms(card: Card, at: &Scaled) -> Forms {
        let [body, caption] = [FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
        let (width, height, shapes, runs) = match card {
            Card::Ride => (
                at.unit(STATUS_UNITS + 8)
                    + status_column(caption, &STATUSES).max(caption.fit_width(b"NO GPS FIX")),
                RIDE_UNITS,
                4,
                4,
            ),
            Card::Sensors => (
                at.unit(2 * INSET_UNITS + (ICONS - 1) * PITCH_UNITS)
                    + usize::from(at.art.sensor(Sensor::of(0), Size::Small).size),
                SENSORS_UNITS,
                2 + 2 * ICONS as u8,
                2 + ICONS as u8,
            ),
            Card::Train | Card::Settings => (
                2 * at.unit(INSET_UNITS)
                    + Form::text(at.face(FaceRole::Caption), card.label().as_bytes()).width
                        as usize,
                SMALL_UNITS,
                2,
                1,
            ),
            Card::Rides => {
                let lines = [this_week(u64::MAX), last_ride(u64::MAX), saved(usize::MAX)];
                let fixed = [NONE_SAVED, UNCOUNTED].map(str::as_bytes);
                let widest = lines
                    .iter()
                    .map(|line| line.as_bytes())
                    .chain(fixed)
                    .map(|line| caption.fit_width(line))
                    .max()
                    .unwrap_or_default();
                (
                    2 * at.unit(INSET_UNITS) + body.width(b"RIDES") + SPILL + at.unit(8) + widest,
                    RIDES_UNITS,
                    2,
                    2,
                )
            }
        };
        forms(&[Form {
            width: width as u16,
            height: at.unit(height) as u16,
            shapes,
            runs,
        }])
    }

    fn prepare(card: Card, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        draw(card, placed, at, data, out, false);
    }

    fn focused(card: Card, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        draw(card, placed, at, data, out, true);
    }

    fn action(card: Card) -> Option<Action> {
        Some(Action::Go(Go::Open(card.opens())))
    }
}
