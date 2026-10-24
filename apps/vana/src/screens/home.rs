//! Home: one big RIDE card, the sensors at a glance, TRAIN (workouts) and
//! settings side by side, and the saved rides with the last one's numbers.
//!
//! MOVE (BottomLeft) moves focus down the cards, ACT (BottomRight) opens the
//! focused one and a tap opens the card under the finger.
use super::chrome::{
    ACCENT, AMBER, Align, BLACK, BODY, CAPTION, CARD, CONTENT, GREEN, Label, Line, MUTED, RAISED,
    Rect, Row, SECONDARY, TITLE, WHITE, blend, card, paint_disc, paint_rounded,
};

/// The width of a card's focus ring, which `paint_rounded` draws per pixel.
const fn ring(focused: bool) -> usize {
    if focused { 3 } else { 0 }
}
use super::sensors::{Glance, icon_row};
use super::workout::Text;
use firmware_shell::rendering::font::{Face, SPILL};
use heapless::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Ride,
    Sensors,
    Workouts,
    Settings,
    Rides,
}

const MARGIN: usize = 10;
const RADIUS: usize = 18;
const GAP: usize = 8;
pub const RIDE_TILE: Rect = Rect {
    left: MARGIN,
    top: CONTENT.top + 6,
    right: CONTENT.right - MARGIN,
    bottom: CONTENT.top + 122,
};
pub const SENSORS_TILE: Rect = Rect {
    left: MARGIN,
    top: RIDE_TILE.bottom + GAP,
    right: CONTENT.right - MARGIN,
    bottom: RIDE_TILE.bottom + GAP + 78,
};
/// TRAIN (the workouts) and SETTINGS share a row, TRAIN first in MOVE
/// order; each card is as wide as its title needs.
pub const WORKOUTS_TILE: Rect = Rect {
    left: MARGIN,
    top: SENSORS_TILE.bottom + GAP,
    right: MARGIN + 90,
    bottom: SENSORS_TILE.bottom + GAP + 40,
};
pub const SETTINGS_TILE: Rect = Rect {
    left: WORKOUTS_TILE.right + GAP,
    top: WORKOUTS_TILE.top,
    right: CONTENT.right - MARGIN,
    bottom: WORKOUTS_TILE.bottom,
};
/// Text inside the half-width cards keeps this far from their edges.
const SMALL_INSET: usize = INSET;
/// The saved rides, as a slim card under settings.
pub const RIDES_TILE: Rect = Rect {
    left: MARGIN,
    top: SETTINGS_TILE.bottom + 6,
    right: CONTENT.right - MARGIN,
    bottom: SETTINGS_TILE.bottom + 6 + 32,
};
const RIDES_RADIUS: usize = 12;
const INSET: usize = 16;
/// The green start button on the RIDE card, centre and radius.
const PLAY: (usize, usize, usize) = (RIDE_TILE.left + 44, RIDE_TILE.top + 58, 28);
/// The text column right of the start button: RIDE, GPS and the status line.
const TEXT_LEFT: usize = PLAY.0 + PLAY.2 + 14;
/// Icons in the sensors card, left to right.
/// Up to four sensors show, each an icon with its short name under it; the
/// count beside the title covers the rest.
const ICONS: usize = 4;
const ICON_PITCH: usize = 52;
const ICON_TOP: usize = SENSORS_TILE.top + 36;
/// The top line of the short names under the icons.
const NAME_TOP: usize = ICON_TOP + 22;

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
pub struct Home {
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
    /// `last` sums up the saved rides, such as the last one's "5.00 KM".
    pub fn new(cursor: usize, gps_fix: bool, glance: &[Glance], last: Text<20>) -> Self {
        let sensors: Vec<Glance, ICONS> = glance.iter().take(ICONS).copied().collect();
        let names = sensors
            .iter()
            .enumerate()
            .map(|(slot, glance)| {
                let centre = SENSORS_TILE.left + INSET + slot * ICON_PITCH + 11;
                let name = short_name(glance.kind);
                label(name, &CAPTION, centre, NAME_TOP, Align::Center, SECONDARY)
            })
            .collect();
        let live = glance.iter().filter(|g| g.live).count();
        let mut count = Text::<12>::new();
        let battery_low = glance.iter().any(Glance::battery_low);
        if battery_low {
            let _ = count.push_str("BATT LOW");
        } else if !glance.is_empty() {
            let _ = core::fmt::Write::write_fmt(
                &mut count,
                format_args!("{live}/{} LIVE", glance.len()),
            );
        }
        let title = |card: usize| if card == cursor { WHITE } else { MUTED };
        let text_left = TEXT_LEFT;
        let (gps_text, gps_color): (&[u8], u16) = if gps_fix {
            (b"GPS READY", GREEN)
        } else {
            (b"NO GPS FIX", SECONDARY)
        };
        // RIDE, the GPS line and the status stack as one block centred on the button.
        let ride_top = PLAY.1 - TITLE.top as usize - TITLE.cap as usize - 5;
        let sensors_top = SENSORS_TILE.top + 14;
        let settings_top = SETTINGS_TILE.top + (SETTINGS_TILE.height() - BODY.height as usize) / 2;
        let left = SENSORS_TILE.left + INSET;
        let right = SENSORS_TILE.right - INSET;
        let count_color = if battery_low {
            AMBER
        } else if live > 0 {
            GREEN
        } else {
            SECONDARY
        };
        let gps_top = PLAY.1;
        let rides_top = RIDES_TILE.top + (RIDES_TILE.height() - BODY.height as usize) / 2;
        let rides_title = label(b"RIDES", &BODY, left, rides_top, Align::Left, title(4));
        // The last ride keeps its numbers: they step down a size to fit.
        let room = right - (rides_title.right() + SPILL + GAP);
        let line = Line::fit(&[&CAPTION], last.as_bytes(), room);
        let last = Label::new(line, right, rides_top + 3, Align::Right, SECONDARY);
        Self {
            cursor,
            ride: label(b"RIDE", &TITLE, text_left, ride_top, Align::Left, title(0)),
            status: Vec::new(),
            gps: label(
                gps_text,
                &CAPTION,
                text_left + 2,
                gps_top,
                Align::Left,
                gps_color,
            ),
            sensors_title: label(b"SENSORS", &BODY, left, sensors_top, Align::Left, title(1)),
            count: label(
                count.as_bytes(),
                &CAPTION,
                right,
                sensors_top + 3,
                Align::Right,
                count_color,
            ),
            none: glance.is_empty().then(|| {
                label(
                    b"NONE PAIRED",
                    &CAPTION,
                    left,
                    ICON_TOP + 8,
                    Align::Left,
                    SECONDARY,
                )
            }),
            sensors,
            names,
            workouts_title: small_title(b"TRAIN", WORKOUTS_TILE, settings_top, title(2)),
            settings_title: small_title(b"SETTINGS", SETTINGS_TILE, settings_top, title(3)),
            rides_title,
            last,
        }
    }

    /// The line under the start button: READY TO RIDE, what is still
    /// starting, or a problem in amber.
    pub fn set_status(&mut self, text: &[u8], problem: bool) {
        let (left, right) = (TEXT_LEFT + 2, RIDE_TILE.right - 8);
        let width = right - left;
        let color = if problem { AMBER } else { SECONDARY };
        let top = PLAY.1 + CAPTION.height as usize + 4;
        // Too long for the column: break at the last space that fits.
        let split = (CAPTION.width(text) + SPILL > width)
            .then(|| {
                (1..text.len())
                    .rev()
                    .find(|&i| text[i] == b' ' && CAPTION.width(&text[..i]) + SPILL <= width)
            })
            .flatten();
        self.status.clear();
        let lines: [&[u8]; 2] = match split {
            Some(i) => [&text[..i], &text[i + 1..]],
            None => [text, b""],
        };
        for (n, part) in lines
            .into_iter()
            .filter(|part| !part.is_empty())
            .enumerate()
        {
            let line = Line::fit(&[&CAPTION], part, width);
            let at = top + n * (CAPTION.height as usize + 2);
            let _ = self
                .status
                .push(Label::new(line, left, at, Align::Left, color));
        }
    }

    /// The card under a tap.
    pub fn tile_at(x: usize, y: usize) -> Option<Tile> {
        if RIDE_TILE.contains(x, y) {
            Some(Tile::Ride)
        } else if SENSORS_TILE.contains(x, y) {
            Some(Tile::Sensors)
        } else if WORKOUTS_TILE.contains(x, y) {
            Some(Tile::Workouts)
        } else if SETTINGS_TILE.contains(x, y) {
            Some(Tile::Settings)
        } else if RIDES_TILE.contains(x, y) {
            Some(Tile::Rides)
        } else {
            None
        }
    }

    /// Paint a row: the card it crosses, then what sits on the card.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        if y < RIDE_TILE.bottom {
            let focused = self.cursor == 0;
            paint_rounded(row, RIDE_TILE, RADIUS, ring(focused), |x, y| {
                Self::surface(focused, RIDE_TILE, x, y)
            });
            if y < RIDE_TILE.top {
                return;
            }
            self.gps.paint(row);
            self.ride.paint(row);
            for status in &self.status {
                status.paint(row);
            }
            let (cx, cy, r) = PLAY;
            let disc = (4 * cx as i32, 4 * cy as i32, 4 * r as i32);
            paint_disc(row, disc, GREEN, |x, button| {
                let fill = Self::surface(focused, RIDE_TILE, x, y)?;
                Some(Self::play(x, y, button, fill))
            });
            // The play triangle: 24 rows, 20 wide at the middle. Its centroid
            // (a third of the way from the flat side) sits on the disc's centre,
            // one pixel right, so it reads as centred.
            let gy = y as i32 - (cy as i32 - 12);
            if (0..24).contains(&gy) {
                let left = cx - 6;
                let width = (20 * (24 - (2 * gy - 23).abs()) / 24) as usize;
                row.span(left..left + width.max(1), BLACK);
            }
        } else if y < SENSORS_TILE.bottom {
            let focused = self.cursor == 1;
            paint_rounded(row, SENSORS_TILE, RADIUS, ring(focused), |x, y| {
                Self::surface(focused, SENSORS_TILE, x, y)
            });
            if y < SENSORS_TILE.top {
                return;
            }
            if let Some(none) = &self.none {
                none.paint(row);
            } else if y >= ICON_TOP {
                self.paint_icons(row, focused);
                for name in &self.names {
                    name.paint(row);
                }
            }
            self.count.paint(row);
            self.sensors_title.paint(row);
        } else if y < SETTINGS_TILE.bottom {
            for (index, tile) in [(2, WORKOUTS_TILE), (3, SETTINGS_TILE)] {
                let focused = self.cursor == index;
                paint_rounded(row, tile, RADIUS, ring(focused), |x, y| {
                    Self::surface(focused, tile, x, y)
                });
            }
            self.workouts_title.paint(row);
            self.settings_title.paint(row);
        } else {
            let focused = self.cursor == 4;
            paint_rounded(row, RIDES_TILE, RIDES_RADIUS, ring(focused), |x, y| {
                Self::surface_of(focused, RIDES_TILE, RIDES_RADIUS, x, y)
            });
            self.rides_title.paint(row);
            self.last.paint(row);
        }
    }

    /// Sensor icons in white, each with its status dot at the top right.
    fn paint_icons(&self, row: &mut Row, focused: bool) {
        let y = row.y();
        let left = SENSORS_TILE.left + INSET;
        for (slot, glance) in self.sensors.iter().enumerate() {
            let slot_left = left + slot * ICON_PITCH;
            let dot = (4 * (slot_left + 25) as i32, 4 * ICON_TOP as i32 + 2, 10);
            paint_disc(row, dot, glance.color, |x, dot| {
                let fill = Self::surface(focused, SENSORS_TILE, x, y)?;
                Some(blend(glance.color, fill, dot))
            });
            row.bits(slot_left, icon_row(glance.kind, y - ICON_TOP), WHITE);
        }
    }

    /// The green start button with a black play triangle, 26 tall, inside it.
    fn play(x: usize, y: usize, button: u8, fill: u16) -> u16 {
        let (cx, cy, _) = PLAY;
        let gx = x as i32 - (cx as i32 - 6);
        let gy = y as i32 - (cy as i32 - 12);
        let inside =
            gx >= 0 && (0..24).contains(&gy) && gx < (20 * (24 - (2 * gy - 23).abs()) / 24).max(1);
        if inside {
            BLACK
        } else {
            blend(GREEN, fill, button)
        }
    }

    /// The per-pixel original of `paint`, kept as the reference it must match.
    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        if y < RIDE_TILE.bottom {
            return self.ride_pixel(x, y);
        }
        if y < SENSORS_TILE.bottom {
            return self.sensors_pixel(x, y);
        }
        if y < SETTINGS_TILE.bottom {
            let (index, tile, title) = if x < SETTINGS_TILE.left {
                (2, WORKOUTS_TILE, &self.workouts_title)
            } else {
                (3, SETTINGS_TILE, &self.settings_title)
            };
            let Some(fill) = Self::surface(self.cursor == index, tile, x, y) else {
                return BLACK;
            };
            return title.pixel(x, y, fill).unwrap_or(fill);
        }
        let Some(fill) = Self::surface_of(self.cursor == 4, RIDES_TILE, RIDES_RADIUS, x, y) else {
            return BLACK;
        };
        for label in [&self.rides_title, &self.last] {
            if let Some(color) = label.pixel(x, y, fill) {
                return color;
            }
        }
        fill
    }

    /// Fill and focus ring: the focused card is raised and ringed.
    fn surface(focused: bool, rect: Rect, x: usize, y: usize) -> Option<u16> {
        Self::surface_of(focused, rect, RADIUS, x, y)
    }

    fn surface_of(focused: bool, rect: Rect, radius: usize, x: usize, y: usize) -> Option<u16> {
        if focused {
            card(rect, radius, x, y, RAISED, Some(ACCENT))
        } else {
            card(rect, radius, x, y, CARD, None)
        }
    }

    #[cfg(test)]
    fn ride_pixel(&self, x: usize, y: usize) -> u16 {
        let Some(fill) = Self::surface(self.cursor == 0, RIDE_TILE, x, y) else {
            return BLACK;
        };
        let (cx, cy, r) = PLAY;
        let button = super::chrome::disc(x, y, 4 * cx as i32, 4 * cy as i32, 4 * r as i32);
        if button > 0 {
            return Self::play(x, y, button, fill);
        }
        for label in [&self.ride, &self.gps].into_iter().chain(&self.status) {
            if let Some(color) = label.pixel(x, y, fill) {
                return color;
            }
        }
        fill
    }

    #[cfg(test)]
    fn sensors_pixel(&self, x: usize, y: usize) -> u16 {
        let Some(fill) = Self::surface(self.cursor == 1, SENSORS_TILE, x, y) else {
            return BLACK;
        };
        for label in [&self.sensors_title, &self.count] {
            if let Some(color) = label.pixel(x, y, fill) {
                return color;
            }
        }
        if let Some(none) = &self.none {
            return none.pixel(x, y, fill).unwrap_or(fill);
        }
        for name in &self.names {
            if let Some(color) = name.pixel(x, y, fill) {
                return color;
            }
        }
        let left = SENSORS_TILE.left + INSET;
        if y < ICON_TOP || x < left {
            return fill;
        }
        let slot = (x - left) / ICON_PITCH;
        let Some(glance) = self.sensors.get(slot) else {
            return fill;
        };
        let (ix, iy) = ((x - left) % ICON_PITCH, y - ICON_TOP);
        if ix < 22 && icon_row(glance.kind, iy) & (1 << ix) != 0 {
            return WHITE;
        }
        // A status dot at the icon's top right.
        let cx = 4 * (left + slot * ICON_PITCH + 25) as i32;
        let dot = super::chrome::disc(x, y, cx, 4 * ICON_TOP as i32 + 2, 10);
        blend(glance.color, fill, dot)
    }
}

/// A half-width card's title: `BODY`, or a caption where `BODY` is too wide.
fn small_title(text: &[u8], tile: Rect, top: usize, color: u16) -> Small {
    let width = tile.width() - 2 * SMALL_INSET;
    let line = Line::fit(&[&BODY, &CAPTION], text, width);
    let top = top + (BODY.height - line.face().height) as usize / 2;
    Label::new(line, tile.left + SMALL_INSET, top, Align::Left, color)
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

    #[test]
    fn focus_raises_and_rings_the_card_and_taps_find_cards() {
        let home = Home::new(0, true, &[], Text::new());
        let inside = (RIDE_TILE.right - 24, RIDE_TILE.bottom - 24);
        assert_eq!(home.pixel(inside.0, inside.1), RAISED);
        assert_eq!(home.pixel(RIDE_TILE.center_x(), RIDE_TILE.top), ACCENT);
        let sensors_inside = (SENSORS_TILE.right - 24, SENSORS_TILE.bottom - 24);
        assert_eq!(home.pixel(sensors_inside.0, sensors_inside.1), CARD);
        assert_eq!(home.pixel(RIDE_TILE.left, RIDE_TILE.top), BLACK, "rounded");
        assert_eq!(home.pixel(PLAY.0 + 20, PLAY.1), GREEN);
        assert_eq!(home.pixel(PLAY.0, PLAY.1), BLACK, "play triangle");
        assert_eq!(home.ride.color, WHITE);
        let sensors = Home::new(1, true, &[], Text::new());
        assert_eq!(sensors.ride.color, MUTED);
        let top = (SENSORS_TILE.center_x(), SENSORS_TILE.top);
        assert_eq!(sensors.pixel(top.0, top.1), ACCENT);
        assert_eq!(Home::tile_at(120, RIDE_TILE.top + 1), Some(Tile::Ride));
        assert_eq!(
            Home::tile_at(120, SENSORS_TILE.bottom - 1),
            Some(Tile::Sensors)
        );
        assert_eq!(Home::tile_at(120, RIDE_TILE.bottom + 2), None);
        assert_eq!(
            Home::tile_at(120, SETTINGS_TILE.top + 2),
            Some(Tile::Settings)
        );
        assert_eq!(
            Home::tile_at(WORKOUTS_TILE.left + 4, WORKOUTS_TILE.top + 20),
            Some(Tile::Workouts)
        );
        let workouts = Home::new(2, true, &[], Text::new());
        let top = (WORKOUTS_TILE.center_x(), WORKOUTS_TILE.top);
        assert_eq!(workouts.pixel(top.0, top.1), ACCENT);
        assert_eq!(workouts.workouts_title.color, WHITE);
        assert_eq!(workouts.settings_title.color, MUTED);
        let settings = Home::new(3, true, &[], Text::new());
        let top = (SETTINGS_TILE.center_x(), SETTINGS_TILE.top);
        assert_eq!(settings.pixel(top.0, top.1), ACCENT);
        assert_eq!(settings.settings_title.color, WHITE);
        assert_eq!(Home::tile_at(2, 100), None);
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
        let home = Home::new(0, false, &glance, last);
        assert_eq!(home.count.line.text(), b"1/2 LIVE");
        assert_eq!(home.count.color, GREEN);
        let mut low = glance;
        low[1].battery = Some(crate::sensors::ant::BatteryStatus::Critical);
        let warned = Home::new(0, false, &low, Text::new());
        assert_eq!(warned.count.line.text(), b"BATT LOW");
        assert_eq!(warned.count.color, AMBER);
        low[1].battery = Some(crate::sensors::ant::BatteryStatus::Ok);
        let fine = Home::new(0, false, &low, Text::new());
        assert_eq!(fine.count.line.text(), b"1/2 LIVE");
        let left = SENSORS_TILE.left + INSET;
        assert_eq!(home.pixel(left + 25, ICON_TOP), GREEN);
        assert_eq!(home.pixel(left + ICON_PITCH + 25, ICON_TOP), 0xfd20);
        assert_eq!(home.last.line.text(), b"5.00 KM");
        assert_eq!(home.rides_title.line.text(), b"RIDES");
        let none = Home::new(0, false, &[], Text::new());
        assert!(none.none.is_some() && none.last.line.text().is_empty());
        // The RIDES card takes the focus and a tap.
        let rides = Home::new(4, false, &[], Text::new());
        assert_eq!(rides.pixel(RIDES_TILE.center_x(), RIDES_TILE.top), ACCENT);
        assert_eq!(Home::tile_at(120, RIDES_TILE.top + 4), Some(Tile::Rides));
    }

    /// What each card shows, as named boxes, for the layout sweep.
    fn cards(home: &Home) -> [(Rect, std::vec::Vec<(std::string::String, Rect)>); 4] {
        use crate::screens::chrome::layout::text;
        let (cx, cy, r) = PLAY;
        let play = Rect {
            left: cx - r,
            top: cy - r,
            right: cx + r + 1,
            bottom: cy + r + 1,
        };
        let mut ride: std::vec::Vec<_> = [&home.ride, &home.gps]
            .into_iter()
            .chain(&home.status)
            .filter_map(text)
            .collect();
        ride.push(("play".into(), play));
        let mut sensors: std::vec::Vec<_> = [&home.sensors_title, &home.count]
            .into_iter()
            .chain(&home.none)
            .chain(&home.names)
            .filter_map(text)
            .collect();
        for slot in 0..home.sensors.len() {
            let left = SENSORS_TILE.left + INSET + slot * ICON_PITCH;
            let icon = Rect {
                left,
                top: ICON_TOP - 1,
                right: left + 28,
                bottom: ICON_TOP + 20,
            };
            sensors.push((std::format!("icon {slot}"), icon));
        }
        [
            (RIDE_TILE, ride),
            (SENSORS_TILE, sensors),
            (
                WORKOUTS_TILE,
                text(&home.workouts_title).into_iter().collect(),
            ),
            (
                SETTINGS_TILE,
                text(&home.settings_title).into_iter().collect(),
            ),
        ]
    }

    #[test]
    fn extreme_home_content_never_overlaps_or_clips() {
        use crate::screens::chrome::layout::{assert_apart, assert_shown, text};
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
        for count in [0, 1, 4, 6] {
            for distance_mm in [0, 15_000_000, 150_000_000, 1_000_000_000] {
                for moving_s in [0, 3_600, 359_999] {
                    {
                        for cursor in 0..5 {
                            let last = if distance_mm == 0 {
                                Text::new()
                            } else {
                                last_ride(distance_mm + moving_s)
                            };
                            let mut home = Home::new(cursor, true, &glances[..count], last.clone());
                            home.set_status(b"SENSORS UNAVAILABLE", true);
                            let what = std::format!("{count} sensors, {:?}", last.as_str());
                            let mut all = std::vec::Vec::new();
                            for (card, items) in cards(&home) {
                                assert_apart(&items, card.inset(4), &what);
                                all.extend(items);
                            }
                            let below = [&home.rides_title, &home.last];
                            let last_items: std::vec::Vec<_> =
                                below.into_iter().filter_map(text).collect();
                            all.extend(last_items.clone());
                            assert_apart(&all, CONTENT, &what);
                            if !last.as_bytes().is_empty() {
                                assert_shown(&last_items, &[last.as_str()], &what);
                            }
                            assert_shown(&all, &["TRAIN", "SETTINGS"], &what);
                        }
                    }
                }
            }
        }
    }
}
