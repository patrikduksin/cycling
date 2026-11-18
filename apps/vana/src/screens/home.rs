//! Home: one big RIDE card, the sensors at a glance, TRAIN (workouts) and
//! settings side by side, and the saved rides with the last one's numbers.
//!
//! MOVE (BottomLeft) moves focus down the cards, ACT (BottomRight) opens the
//! focused one and a tap opens the card under the finger.
use super::Layout;
#[cfg(test)]
use super::chrome::Look;
use super::chrome::{
    AMBER, Align, BLACK, GREEN, Inks, Label, Line, MUTED, Rect, Row, SECONDARY, Shown, WHITE,
    blend, card, paint_disc, paint_rounded,
};
use super::icons::{Icon, Sensor, Size};
use super::scaled::{Scaled, Track};
use super::sensors::Glance;
use super::workout::Text;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};
use heapless::Vec;

/// The width of a card's focus ring, which `paint_rounded` fills as spans
/// where it is solid.
const fn ring(focused: bool) -> usize {
    if focused { 3 } else { 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Ride,
    Sensors,
    Workouts,
    Settings,
    Rides,
}

const RADIUS: usize = 18;
const RIDES_RADIUS: usize = 12;
const GAP: usize = 8;
/// Text inside the half-width cards keeps this far from their edges.
const SMALL_INSET: usize = 16;
/// Up to four sensors show, each an icon with its short name under it; the
/// count beside the title covers the rest.
const ICONS: usize = 4;

/// Home on one panel: the cards top to bottom, and where each card's
/// contents sit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The rider's first card takes a taller panel's spare rows.
    pub ride: Rect,
    pub sensors: Rect,
    /// TRAIN (the workouts) and SETTINGS share a row, TRAIN first in MOVE
    /// order; each card is as wide as its title needs.
    pub workouts: Rect,
    pub settings: Rect,
    /// The saved rides, as a slim card under settings.
    pub rides: Rect,
    /// The green start button on the RIDE card: centre and radius.
    pub play: (usize, usize, usize),
    /// The play triangle's box, its flat side on the left. Its centroid (a
    /// third of the way from the flat side) sits on the button's centre, one
    /// pixel right, so it reads as centred.
    pub triangle: Rect,
    /// RIDE, its capitals ending on the bottom, right of the start button.
    pub ride_title: Rect,
    /// The GPS line, then what is still starting on one or two lines.
    pub gps: Rect,
    pub status: [Rect; 2],
    /// The sensors card's title row, inset; the live count ends on its right.
    pub sensors_title: Rect,
    /// Each sensor's icon square, left to right; its status dot sits at the
    /// square's top right.
    pub slots: [Rect; ICONS],
    /// The short names under the icons, centred on them but kept clear of
    /// the focus ring.
    pub names: Rect,
    /// RIDES and the last ride's numbers, inset like the sensors.
    pub rides_text: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Line, Unit};
    let content = super::chrome::regions(at).content;
    let [_, cards, _] = content.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    // The bottom margin is split off alone: after the units above it, it
    // rounds to 1 px at 3/4; alone it keeps the C606's 2 px.
    let [cards, _] = cards.rows([Fr(1), Unit(2)], Unit(0), at);
    let [_, ride, _, sensors, _, small, _, rides] = cards.rows(
        [
            Unit(6),
            Fr(1),
            Unit(8),
            Unit(78),
            Unit(8),
            Unit(40),
            Unit(6),
            Unit(32),
        ],
        Unit(0),
        at,
    );
    let [workouts, _, settings] = small.cols([Unit(90), Unit(GAP as u16), Fr(1)], Unit(0), at);

    let [_, button, _, title, text, _] = ride.cols(
        [Unit(16), Unit(56), Unit(14), Unit(2), Fr(1), Unit(8)],
        Unit(0),
        at,
    );
    let caption = Line(FaceRole::Caption);
    let [upper, lower] = ride.rows([Fr(1), Fr(1)], Unit(0), at);
    let [above, _] = upper.rows([Fr(1), Unit(5)], Unit(0), at);
    let [gps, _, first, _, second, _] = lower.rows(
        [caption, Unit(4), caption, Unit(2), caption, Fr(1)],
        Unit(0),
        at,
    );
    let (cx, cy, r) = (
        (button.left + button.right) / 2,
        gps.top,
        button.width() / 2,
    );
    let (left, top) = (cx - at.unit(6), cy - at.unit(12));

    let [_, inner, _] = sensors.cols([Unit(16), Fr(1), Unit(16)], Unit(0), at);
    let [_, sensors_title, icons, _, names, _] = inner.rows(
        [Unit(14), Unit(21), Unit(22), Unit(1), caption, Fr(1)],
        Unit(0),
        at,
    );
    let [_, clear, _] = sensors.cols([Unit(5), Fr(1), Unit(5)], Unit(0), at);
    let pitch = Unit(52);
    Regions {
        ride,
        sensors,
        workouts,
        settings,
        rides,
        play: (cx, cy, r),
        triangle: Rect {
            left,
            top,
            right: left + at.unit(20),
            bottom: top + at.unit(24),
        },
        ride_title: Rect {
            left: title.left,
            right: text.right,
            ..above
        },
        gps: gps.with_columns(text),
        status: [first.with_columns(text), second.with_columns(text)],
        sensors_title,
        slots: icons.cols([pitch, pitch, pitch, Fr(1)], Unit(0), at),
        names: names.with_columns(clear),
        rides_text: rides.with_columns(inner),
    }
}

impl Regions {
    /// The card under a tap.
    pub const fn tile_at(&self, x: usize, y: usize) -> Option<Tile> {
        if self.ride.contains(x, y) {
            Some(Tile::Ride)
        } else if self.sensors.contains(x, y) {
            Some(Tile::Sensors)
        } else if self.workouts.contains(x, y) {
            Some(Tile::Workouts)
        } else if self.settings.contains(x, y) {
            Some(Tile::Settings)
        } else if self.rides.contains(x, y) {
            Some(Tile::Rides)
        } else {
            None
        }
    }
}

type Small = Label<20>;

const fn label(
    text: &[u8],
    face: &'static Face,
    x: usize,
    top: usize,
    align: Align,
    color: u16,
) -> Small {
    Label::new(Line::new(face, text), x, top, align, color)
}

/// What Home shows, prepared once per frame.
#[derive(Hash)]
pub struct Home {
    at: &'static Scaled,
    regions: &'static Regions,
    /// The focused card: Ride, Sensors or Settings.
    pub cursor: usize,
    ride: Small,
    gps: Small,
    /// What is still starting, or why turning off did not happen.
    /// One line, or two when it is too long for the text column.
    status: Vec<Small, 2>,
    sensors_title: Small,
    count: Small,
    none: Option<Small>,
    sensors: Vec<Glance, ICONS>,
    names: Vec<Small, ICONS>,
    workouts_title: Small,
    settings_title: Small,
    rides_title: Small,
    last: Small,
}

impl Home {
    /// Home laid out on `layout`'s panel. `last` sums up the saved rides,
    /// such as the last one's "5.00 KM".
    pub fn new(
        layout: &'static Layout,
        cursor: usize,
        gps_fix: bool,
        glance: &[Glance],
        last: Text<20>,
    ) -> Self {
        let (at, regions) = (&layout.scaled, &layout.home);
        let face = |role| at.face(role);
        let (caption, body) = (face(FaceRole::Caption), face(FaceRole::Body));
        let sensors: Vec<Glance, ICONS> = glance.iter().take(ICONS).copied().collect();
        let names = sensors
            .iter()
            .zip(&regions.slots)
            .map(|(glance, slot)| {
                let centre = slot.left + usize::from(icon(at, glance).size) / 2;
                let name = short_name(glance.kind);
                let top = regions.names.top;
                label(name, caption, centre, top, Align::Center, SECONDARY)
            })
            .collect();
        let live = glance.iter().filter(|g| g.live).count();
        let mut count = Text::<12>::new();
        let battery_low = glance.iter().any(|g| g.live && g.battery_low());
        if !glance.is_empty() {
            let _ = core::fmt::Write::write_fmt(
                &mut count,
                format_args!("{live}/{} LIVE", glance.len()),
            );
        }
        let title = |card: usize| if card == cursor { WHITE } else { MUTED };
        let (gps_text, gps_color): (&[u8], u16) = if gps_fix {
            (b"GPS READY", GREEN)
        } else {
            (b"NO GPS FIX", SECONDARY)
        };
        let ride_face = face(FaceRole::Title);
        let ride = regions.ride_title;
        let ride_top = ride.bottom - ride_face.top as usize - ride_face.cap as usize;
        let header = regions.sensors_title;
        let small = regions.settings;
        let small_top = small.top + (small.height() - body.height as usize) / 2;
        let count_color = if battery_low {
            AMBER
        } else if live > 0 {
            GREEN
        } else {
            SECONDARY
        };
        let rides = regions.rides_text;
        let rides_top = rides.top + (rides.height() - body.height as usize) / 2;
        let rides_title = label(b"RIDES", body, rides.left, rides_top, Align::Left, title(4));
        // The last ride keeps its numbers: they step down a size to fit.
        let room = rides.right - (rides_title.right() + SPILL + at.unit(GAP));
        let line = Line::fit(&[caption], last.as_bytes(), room);
        let last = Label::new(
            line,
            rides.right,
            rides_top + at.unit(3),
            Align::Right,
            SECONDARY,
        );
        let gps = regions.gps;
        let titled = |text, tile, color| small_title(at, text, tile, small_top, color);
        Self {
            at,
            regions,
            cursor,
            ride: label(
                b"RIDE",
                ride_face,
                ride.left,
                ride_top,
                Align::Left,
                title(0),
            ),
            status: Vec::new(),
            gps: label(gps_text, caption, gps.left, gps.top, Align::Left, gps_color),
            sensors_title: label(
                b"SENSORS",
                body,
                header.left,
                header.top,
                Align::Left,
                title(1),
            ),
            count: label(
                count.as_bytes(),
                caption,
                header.right,
                header.top + at.unit(3),
                Align::Right,
                count_color,
            ),
            none: glance.is_empty().then(|| {
                label(
                    b"NONE PAIRED",
                    caption,
                    header.left,
                    regions.slots[0].top + at.unit(9),
                    Align::Left,
                    SECONDARY,
                )
            }),
            sensors,
            names,
            workouts_title: titled(b"TRAIN", regions.workouts, title(2)),
            settings_title: titled(b"SETTINGS", regions.settings, title(3)),
            rides_title,
            last,
        }
    }

    /// The line under the start button: READY TO RIDE, what is still
    /// starting, or a problem in amber.
    pub fn set_status(&mut self, text: &[u8], problem: bool) {
        self.set_status_in(text, if problem { AMBER } else { SECONDARY });
    }

    /// The status line in a colour of its own, such as red for a critical
    /// battery.
    pub fn set_status_in(&mut self, text: &[u8], color: u16) {
        let caption = self.at.face(FaceRole::Caption);
        let rows = self.regions.status;
        let width = rows[0].width();
        // Too long for the column: break at the last space that fits.
        let split = (caption.width(text) + SPILL > width)
            .then(|| {
                (1..text.len())
                    .rev()
                    .find(|&i| text[i] == b' ' && caption.width(&text[..i]) + SPILL <= width)
            })
            .flatten();
        self.status.clear();
        let lines: [&[u8]; 2] = match split {
            Some(i) => [&text[..i], &text[i + 1..]],
            None => [text, b""],
        };
        for (part, row) in lines.into_iter().filter(|part| !part.is_empty()).zip(rows) {
            let line = Line::fit(&[caption], part, width);
            let _ = self
                .status
                .push(Label::new(line, row.left, row.top, Align::Left, color));
        }
    }

    /// Paint a row: the card it crosses, then what sits on the card.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        let inks = Inks::of(row.look());
        let regions = self.regions;
        let radius = self.at.unit(RADIUS);
        if y < regions.ride.bottom {
            let focused = self.cursor == 0;
            let tile = regions.ride;
            paint_rounded(row, tile, radius, ring(focused), |x, y| {
                surface(inks, focused, tile, radius, x, y)
            });
            if y < tile.top {
                return;
            }
            self.gps.paint(row);
            self.ride.paint(row);
            for status in &self.status {
                status.paint(row);
            }
            let (cx, cy, r) = regions.play;
            let disc = (4 * cx as i32, 4 * cy as i32, 4 * r as i32);
            paint_disc(row, disc, GREEN, |x, button, _, _| {
                let fill = surface(inks, focused, tile, radius, x, y)?;
                Some(self.play(inks, x, y, button, fill))
            });
            if let Some(width) = triangle_width(regions.triangle, y) {
                let left = regions.triangle.left;
                row.span(left..left + width.max(1), BLACK);
            }
        } else if y < regions.sensors.bottom {
            let focused = self.cursor == 1;
            let tile = regions.sensors;
            paint_rounded(row, tile, radius, ring(focused), |x, y| {
                surface(inks, focused, tile, radius, x, y)
            });
            if y < tile.top {
                return;
            }
            if let Some(none) = &self.none {
                none.paint(row);
            } else if y >= regions.slots[0].top {
                self.paint_icons(row, inks, focused);
                for name in &self.names {
                    name.paint(row);
                }
            }
            self.count.paint(row);
            self.sensors_title.paint(row);
        } else if y < regions.settings.bottom {
            for (index, tile) in [(2, regions.workouts), (3, regions.settings)] {
                let focused = self.cursor == index;
                paint_rounded(row, tile, radius, ring(focused), |x, y| {
                    surface(inks, focused, tile, radius, x, y)
                });
            }
            self.workouts_title.paint(row);
            self.settings_title.paint(row);
        } else {
            let focused = self.cursor == 4;
            let (tile, radius) = (regions.rides, self.at.unit(RIDES_RADIUS));
            paint_rounded(row, tile, radius, ring(focused), |x, y| {
                surface(inks, focused, tile, radius, x, y)
            });
            self.rides_title.paint(row);
            self.last.paint(row);
        }
    }

    /// Sensor icons in white, each with its status dot at the top right.
    fn paint_icons(&self, row: &mut Row, inks: Inks, focused: bool) {
        let y = row.y();
        let tile = self.regions.sensors;
        let radius = self.at.unit(RADIUS);
        for (glance, slot) in self.sensors.iter().zip(&self.regions.slots) {
            paint_disc(row, self.dot(*slot), glance.color, |x, dot, color, _| {
                let fill = surface(inks, focused, tile, radius, x, y)?;
                Some(blend(color, fill, dot))
            });
            icon(self.at, glance).paint(row, slot.left, slot.top, WHITE);
        }
    }

    fn dot(&self, slot: Rect) -> (i32, i32, i32) {
        let at = self.at;
        (
            (4 * slot.left + at.unit(100)) as i32,
            (4 * slot.top + at.unit(6)) as i32,
            at.unit(10) as i32,
        )
    }

    fn play(&self, inks: Inks, x: usize, y: usize, button: u8, fill: Shown) -> Shown {
        let triangle = self.regions.triangle;
        let inside = x >= triangle.left
            && triangle_width(triangle, y).is_some_and(|width| x - triangle.left < width.max(1));
        if inside {
            inks.black
        } else {
            blend(inks.green, fill, button)
        }
    }

    #[cfg(test)]
    pub fn pixel(&self, look: Look, x: usize, y: usize) -> Shown {
        let inks = Inks::of(look);
        let regions = self.regions;
        if y < regions.ride.bottom {
            return self.ride_pixel(look, x, y);
        }
        if y < regions.sensors.bottom {
            return self.sensors_pixel(look, x, y);
        }
        let radius = self.at.unit(RADIUS);
        if y < regions.settings.bottom {
            let (index, tile, title) = if x < regions.settings.left {
                (2, regions.workouts, &self.workouts_title)
            } else {
                (3, regions.settings, &self.settings_title)
            };
            let Some(fill) = surface(inks, self.cursor == index, tile, radius, x, y) else {
                return inks.black;
            };
            return title.pixel(look, x, y, fill).unwrap_or(fill);
        }
        let radius = self.at.unit(RIDES_RADIUS);
        let Some(fill) = surface(inks, self.cursor == 4, regions.rides, radius, x, y) else {
            return inks.black;
        };
        for label in [&self.rides_title, &self.last] {
            if let Some(color) = label.pixel(look, x, y, fill) {
                return color;
            }
        }
        fill
    }

    #[cfg(test)]
    fn ride_pixel(&self, look: Look, x: usize, y: usize) -> Shown {
        let inks = Inks::of(look);
        let radius = self.at.unit(RADIUS);
        let Some(fill) = surface(inks, self.cursor == 0, self.regions.ride, radius, x, y) else {
            return inks.black;
        };
        let (cx, cy, r) = self.regions.play;
        let button = super::chrome::disc(x, y, 4 * cx as i32, 4 * cy as i32, 4 * r as i32);
        if button > 0 {
            return self.play(inks, x, y, button, fill);
        }
        for label in [&self.ride, &self.gps].into_iter().chain(&self.status) {
            if let Some(color) = label.pixel(look, x, y, fill) {
                return color;
            }
        }
        fill
    }

    #[cfg(test)]
    fn sensors_pixel(&self, look: Look, x: usize, y: usize) -> Shown {
        let inks = Inks::of(look);
        let radius = self.at.unit(RADIUS);
        let tile = self.regions.sensors;
        let Some(fill) = surface(inks, self.cursor == 1, tile, radius, x, y) else {
            return inks.black;
        };
        for label in [&self.sensors_title, &self.count] {
            if let Some(color) = label.pixel(look, x, y, fill) {
                return color;
            }
        }
        if let Some(none) = &self.none {
            return none.pixel(look, x, y, fill).unwrap_or(fill);
        }
        for name in &self.names {
            if let Some(color) = name.pixel(look, x, y, fill) {
                return color;
            }
        }
        let slots = &self.regions.slots;
        if y < slots[0].top {
            return fill;
        }
        let Some((glance, slot)) = self
            .sensors
            .iter()
            .zip(slots)
            .find(|(_, slot)| (slot.left..slot.right).contains(&x))
        else {
            return fill;
        };
        let (cx, cy, r) = self.dot(*slot);
        let dot = super::chrome::disc(x, y, cx, cy, r);
        let under = blend(look.ink(glance.color), fill, dot);
        icon(self.at, glance)
            .pixel(x, y, slot.left, slot.top, look.ink(WHITE), under)
            .unwrap_or(under)
    }
}

#[cfg(any(test, feature = "invariants"))]
pub(crate) type Card = (
    &'static str,
    Rect,
    std::vec::Vec<(std::string::String, Rect)>,
);

#[cfg(any(test, feature = "invariants"))]
impl Home {
    pub(crate) fn cards(&self) -> [Card; 5] {
        use super::chrome::layout::text;
        let regions = self.regions;
        let (cx, cy, r) = regions.play;
        let play = Rect {
            left: cx - r,
            top: cy - r,
            right: cx + r + 1,
            bottom: cy + r + 1,
        };
        let mut ride: std::vec::Vec<_> = [&self.ride, &self.gps]
            .into_iter()
            .chain(&self.status)
            .filter_map(text)
            .collect();
        ride.push(("play".into(), play));
        let mut sensors: std::vec::Vec<_> = [&self.sensors_title, &self.count]
            .into_iter()
            .chain(&self.none)
            .chain(&self.names)
            .filter_map(text)
            .collect();
        for (slot, (glance, square)) in self.sensors.iter().zip(&regions.slots).enumerate() {
            let ink = icon(self.at, glance).ink(square.left, square.top);
            let (dx, dy, dr) = self.dot(*square);
            let dot = |quarters: i32| (quarters / 4) as usize;
            let icon = Rect {
                left: ink.left.min(dot(dx - dr)),
                top: ink.top.min(dot(dy - dr)),
                right: ink.right.max(dot(dx + dr) + 1),
                bottom: ink.bottom.max(dot(dy + dr) + 1),
            };
            sensors.push((std::format!("icon {slot}"), icon));
        }
        let rides = [&self.rides_title, &self.last]
            .into_iter()
            .filter_map(text)
            .collect();
        [
            ("ride card", regions.ride, ride),
            ("sensors card", regions.sensors, sensors),
            (
                "train card",
                regions.workouts,
                text(&self.workouts_title).into_iter().collect(),
            ),
            (
                "settings card",
                regions.settings,
                text(&self.settings_title).into_iter().collect(),
            ),
            ("rides card", regions.rides, rides),
        ]
    }
}

fn triangle_width(triangle: Rect, y: usize) -> Option<usize> {
    let (rows, wide) = (triangle.height() as i32, triangle.width() as i32);
    let gy = y.checked_sub(triangle.top)? as i32;
    (gy < rows).then(|| (wide * (rows - (2 * gy - (rows - 1)).abs()) / rows) as usize)
}

fn icon(at: &Scaled, glance: &Glance) -> &'static Icon {
    at.art.sensor(Sensor::of(glance.kind), Size::Small)
}

fn surface(
    inks: Inks,
    focused: bool,
    rect: Rect,
    radius: usize,
    x: usize,
    y: usize,
) -> Option<Shown> {
    if focused {
        card(
            rect,
            radius,
            x,
            y,
            inks.raised,
            Some(inks.accent),
            inks.black,
        )
    } else {
        card(rect, radius, x, y, inks.card, None, inks.black)
    }
}

fn small_title(at: &Scaled, text: &[u8], tile: Rect, top: usize, color: u16) -> Small {
    let body = at.face(FaceRole::Body);
    let inset = at.unit(SMALL_INSET);
    let width = tile.width() - 2 * inset;
    let line = Line::fit(&[body, at.face(FaceRole::Caption)], text, width);
    let top = top + (body.height - line.face().height) as usize / 2;
    Label::new(line, tile.left + inset, top, Align::Left, color)
}

/// A sensor's name short enough to sit under its icon on Home.
pub(crate) fn short_name(kind: u8) -> &'static [u8] {
    use crate::sensors::ant::device_type;
    match kind {
        device_type::RADAR => b"RDR",
        device_type::POWER => b"PWR",
        device_type::HEART_RATE => b"HR",
        device_type::SPEED_CADENCE => b"SPD/C",
        device_type::CADENCE => b"CAD",
        device_type::SPEED => b"SPD",
        device_type::SHIFTING | device_type::SHIFTING_DI2 => b"SHIFT",
        device_type::FITNESS_EQUIPMENT => b"FIT",
        device_type::BIKE_LIGHTS => b"LIGHT",
        _ => b"ANT",
    }
}

/// "5.00 KM": the last ride's distance, beside RIDES.
pub fn last_ride(distance_mm: u64) -> Text<20> {
    let mut km = Text::<12>::new();
    super::ride::kilometres(&mut km, distance_mm);
    let mut line = Text::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("{} KM", km.as_str()));
    line
}

/// "WEEK 84 KM": the week's distance, beside RIDES (THIS WEEK is too wide).
pub fn this_week(distance_mm: u64) -> Text<20> {
    let mut line = Text::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!("WEEK {} KM", (distance_mm + 500_000) / 1_000_000),
    );
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::BASE;
    use crate::screens::chrome::{ACCENT, CARD, RAISED};

    const R: Regions = BASE.home;

    fn home(cursor: usize, gps_fix: bool, glance: &[Glance], last: Text<20>) -> Home {
        Home::new(&BASE, cursor, gps_fix, glance, last)
    }

    #[test]
    fn the_c606_keeps_its_cards() {
        let rect = |left, top, right, bottom| Rect {
            left,
            top,
            right,
            bottom,
        };
        assert_eq!(R.ride, rect(10, 30, 230, 146));
        assert_eq!(R.sensors, rect(10, 154, 230, 232));
        assert_eq!(R.workouts, rect(10, 240, 100, 280));
        assert_eq!(R.settings, rect(108, 240, 230, 280));
        assert_eq!(R.rides, rect(10, 286, 230, 318));
        assert_eq!(R.play, (54, 88, 28));
        assert_eq!(R.triangle, rect(48, 76, 68, 100));
        assert_eq!((R.ride_title.left, R.ride_title.bottom), (96, 83));
        assert_eq!((R.gps.left, R.gps.top), (98, 88));
        assert_eq!(
            R.status.map(|row| (row.left, row.top, row.right)),
            [(98, 106, 222), (98, 122, 222)]
        );
        assert_eq!((R.sensors_title.left, R.sensors_title.top), (26, 168));
        assert_eq!(
            R.slots.map(|slot| (slot.left, slot.top)),
            [(26, 189), (78, 189), (130, 189), (182, 189)]
        );
        assert_eq!(R.names.top, 212);
    }

    #[test]
    fn focus_raises_and_rings_the_card_and_taps_find_cards() {
        let home = home(0, true, &[], Text::new());
        let inside = (R.ride.right - 24, R.ride.bottom - 24);
        assert_eq!(home.pixel(Look::PLAIN, inside.0, inside.1).0, RAISED);
        assert_eq!(
            home.pixel(Look::PLAIN, R.ride.center_x(), R.ride.top).0,
            ACCENT
        );
        let sensors_inside = (R.sensors.right - 24, R.sensors.bottom - 24);
        assert_eq!(
            home.pixel(Look::PLAIN, sensors_inside.0, sensors_inside.1)
                .0,
            CARD
        );
        assert_eq!(
            home.pixel(Look::PLAIN, R.ride.left, R.ride.top).0,
            BLACK,
            "rounded"
        );
        let (cx, cy, _) = R.play;
        assert_eq!(home.pixel(Look::PLAIN, cx + 20, cy).0, GREEN);
        assert_eq!(home.pixel(Look::PLAIN, cx, cy).0, BLACK, "play triangle");
        assert_eq!(home.ride.color, WHITE);
        let sensors = home_at(1);
        assert_eq!(sensors.ride.color, MUTED);
        let top = (R.sensors.center_x(), R.sensors.top);
        assert_eq!(sensors.pixel(Look::PLAIN, top.0, top.1).0, ACCENT);
        assert_eq!(R.tile_at(120, R.ride.top + 1), Some(Tile::Ride));
        assert_eq!(R.tile_at(120, R.sensors.bottom - 1), Some(Tile::Sensors));
        assert_eq!(R.tile_at(120, R.ride.bottom + 2), None);
        assert_eq!(R.tile_at(120, R.settings.top + 2), Some(Tile::Settings));
        assert_eq!(
            R.tile_at(R.workouts.left + 4, R.workouts.top + 20),
            Some(Tile::Workouts)
        );
        let workouts = home_at(2);
        let top = (R.workouts.center_x(), R.workouts.top);
        assert_eq!(workouts.pixel(Look::PLAIN, top.0, top.1).0, ACCENT);
        assert_eq!(workouts.workouts_title.color, WHITE);
        assert_eq!(workouts.settings_title.color, MUTED);
        let settings = home_at(3);
        let top = (R.settings.center_x(), R.settings.top);
        assert_eq!(settings.pixel(Look::PLAIN, top.0, top.1).0, ACCENT);
        assert_eq!(settings.settings_title.color, WHITE);
        assert_eq!(R.tile_at(2, 100), None);
    }

    fn home_at(cursor: usize) -> Home {
        home(cursor, true, &[], Text::new())
    }

    #[test]
    fn taps_find_the_cards_on_every_panel() {
        for layout in crate::screens::panels::ALL {
            let regions = &layout.home;
            for (tile, rect) in [
                (Tile::Ride, regions.ride),
                (Tile::Sensors, regions.sensors),
                (Tile::Workouts, regions.workouts),
                (Tile::Settings, regions.settings),
                (Tile::Rides, regions.rides),
            ] {
                let (x, y) = (rect.center_x(), rect.top + rect.height() / 2);
                assert_eq!(regions.tile_at(x, y), Some(tile), "{:?}", layout.scaled);
            }
            let between = regions.ride.bottom + 1;
            assert_eq!(regions.tile_at(regions.ride.center_x(), between), None);
        }
    }

    #[test]
    fn sensors_card_counts_live_sensors_and_shows_their_state() {
        let glance = [
            Glance {
                kind: 120,
                live: true,
                color: GREEN,
                battery: None,
            },
            Glance {
                kind: 11,
                live: false,
                color: 0xfd20,
                battery: None,
            },
        ];
        assert_eq!(this_week(84_400_000).as_bytes(), b"WEEK 84 KM");
        assert_eq!(this_week(499_999).as_bytes(), b"WEEK 0 KM");
        let last = last_ride(4_999_800);
        assert_eq!(last.as_bytes(), b"5.00 KM");
        let home_with = |glance: &[Glance], last| home(0, false, glance, last);
        let shown = home_with(&glance, last);
        assert_eq!(shown.count.line.text(), b"1/2 LIVE");
        assert_eq!(shown.count.color, GREEN);
        let mut low = glance;
        // A dead strap is not live, so its battery is not the news.
        low[1].battery = Some(crate::sensors::ant::BatteryStatus::Critical);
        assert_eq!(home_with(&low, Text::new()).count.color, GREEN);
        low[0].battery = Some(crate::sensors::ant::BatteryStatus::Low);
        let warned = home_with(&low, Text::new());
        assert_eq!(warned.count.line.text(), b"1/2 LIVE");
        assert_eq!(warned.count.color, AMBER);
        low[0].battery = Some(crate::sensors::ant::BatteryStatus::Ok);
        assert_eq!(home_with(&low, Text::new()).count.color, GREEN);
        let dot_top = R.slots[0].top + 1;
        assert_eq!(
            shown.pixel(Look::PLAIN, R.slots[0].left + 25, dot_top).0,
            GREEN
        );
        assert_eq!(
            shown.pixel(Look::PLAIN, R.slots[1].left + 25, dot_top).0,
            0xfd20
        );
        assert_eq!(shown.last.line.text(), b"5.00 KM");
        assert_eq!(shown.rides_title.line.text(), b"RIDES");
        let none = home_with(&[], Text::new());
        assert!(none.none.is_some() && none.last.line.text().is_empty());
        // The RIDES card takes the focus and a tap.
        let rides = home_at(4);
        assert_eq!(
            rides.pixel(Look::PLAIN, R.rides.center_x(), R.rides.top).0,
            ACCENT
        );
        assert_eq!(R.tile_at(120, R.rides.top + 4), Some(Tile::Rides));
    }

    #[test]
    fn extreme_home_content_never_overlaps_or_clips() {
        use crate::screens::chrome::layout::{assert_apart, assert_shown};
        use crate::screens::workout::{Page, View};
        use crate::sensors::ant::device_type;
        let kinds = [
            device_type::SPEED_CADENCE,
            device_type::BIKE_LIGHTS,
            device_type::SPEED_CADENCE,
            device_type::POWER,
            device_type::HEART_RATE,
            device_type::RADAR,
        ];
        let glances: std::vec::Vec<Glance> = kinds
            .iter()
            .enumerate()
            .map(|(index, &kind)| Glance {
                kind,
                live: true,
                color: GREEN,
                battery: (index == 1).then_some(crate::sensors::ant::BatteryStatus::Low),
            })
            .collect();
        crate::screens::check::enable(true);
        for layout in crate::screens::panels::ALL {
            for count in [0, 1, 4, 6] {
                for distance_mm in [0, 15_000_000, 150_000_000, 1_000_000_000] {
                    for cursor in 0..5 {
                        let last = if distance_mm == 0 {
                            Text::new()
                        } else {
                            last_ride(distance_mm)
                        };
                        let mut home =
                            Home::new(layout, cursor, true, &glances[..count], last.clone());
                        let status = [&b"SENSORS UNAVAILABLE"[..], b"NOT SAVED"][cursor % 2];
                        home.set_status(status, true);
                        let what = std::format!(
                            "{:?}: {count} sensors, {:?}",
                            layout.scaled,
                            last.as_str()
                        );
                        let lines: std::vec::Vec<&[u8]> =
                            home.status.iter().map(|label| label.line.text()).collect();
                        assert_eq!(lines.join(&b' '), status, "{what}: the whole status");
                        let ring = layout.scaled.unit(4);
                        let mut all = std::vec::Vec::new();
                        for (_, card, items) in home.cards() {
                            assert_apart(&items, card.inset(ring), &what);
                            all.extend(items);
                        }
                        if !last.as_bytes().is_empty() {
                            assert_shown(&all, &[last.as_str()], &what);
                        }
                        assert_shown(&all, &["RIDE", "TRAIN", "SETTINGS", "RIDES"], &what);
                        let mut view = View::new(Page::Home, cursor, None);
                        view.layout = layout;
                        view.set_home(home);
                        view.prepare().check_layout();
                    }
                }
            }
        }
    }
}
