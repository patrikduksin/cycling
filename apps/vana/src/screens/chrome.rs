//! Shared chrome: canvas geometry, palette, shapes, the status bar and toasts.
//!
//! Every screen draws below the status bar inside [`CONTENT`]; pages derive
//! their own boxes from it rather than hard-coding coordinates, so the chrome
//! can change without rewriting them. See `docs/ui.md`.
use super::workout::Text;
use core::ops::Range;
pub use firmware_shell::rendering::font::{
    Align, BODY, CAPTION, HERO, LARGE, Label, Line, TITLE, VALUE, blend,
};
pub use firmware_shell::rendering::row::Row;

pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 320;

/// A half-open box on the 240 × 320 canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
}

impl Rect {
    pub const fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub const fn width(&self) -> usize {
        self.right - self.left
    }

    pub const fn height(&self) -> usize {
        self.bottom - self.top
    }

    pub const fn center_x(&self) -> usize {
        (self.left + self.right) / 2
    }

    /// The same box shrunk by `by` on every side.
    pub const fn inset(&self, by: usize) -> Self {
        Self {
            left: self.left + by,
            top: self.top + by,
            right: self.right - by,
            bottom: self.bottom - by,
        }
    }
}

/// The thin bar on top of every screen but the boot splash. It has no
/// divider: black space separates it from the content.
pub const STATUS_BAR: Rect = Rect {
    left: 0,
    top: 0,
    right: WIDTH,
    bottom: 22,
};
/// Everything below the status bar.
pub const CONTENT: Rect = Rect {
    left: 0,
    top: STATUS_BAR.bottom + 2,
    right: WIDTH,
    bottom: HEIGHT,
};
/// Toasts float over the bottom of the content; pages keep their least
/// important readings, or nothing, here.
pub const TOAST: Rect = Rect {
    left: 0,
    top: HEIGHT - 40,
    right: WIDTH,
    bottom: HEIGHT,
};

/// RGB565 from 8-bit channels.
pub const fn rgb(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

// Palette. The background is black; colour carries meaning, never decoration.
pub const BLACK: u16 = 0;
/// Primary text and neutral values (distance, time).
pub const WHITE: u16 = 0xffff;
/// Labels, units and secondary text.
pub const SECONDARY: u16 = rgb(0x8e, 0x8e, 0x93);
/// Unfocused titles.
pub const MUTED: u16 = rgb(0xae, 0xae, 0xb2);
/// Unlit dots and gauge segments.
pub const TERTIARY: u16 = rgb(0x48, 0x48, 0x4a);
/// Grouping cards.
pub const CARD: u16 = rgb(0x1c, 0x1c, 0x1e);
/// A focused card, buttons and toasts.
pub const RAISED: u16 = rgb(0x2c, 0x2c, 0x2e);
/// A card under a resting finger, as iOS highlights a touched cell (dark
/// mode's systemGray4).
pub const PRESSED: u16 = rgb(0x3a, 0x3a, 0x3c);
/// Focus rings and information.
pub const ACCENT: u16 = rgb(0x0a, 0x84, 0xff);
/// Live, good, fixed, go.
pub const GREEN: u16 = rgb(0x30, 0xd1, 0x58);
/// Attention: paused, low, needs a check.
pub const AMBER: u16 = rgb(0xff, 0x9f, 0x0a);
/// Recording, danger, failure.
pub const RED: u16 = rgb(0xff, 0x45, 0x3a);

// Signature colours: each metric's value always has its own.
pub const SPEED: u16 = rgb(0x64, 0xd2, 0xff);
pub const POWER: u16 = rgb(0xff, 0xd6, 0x0a);
pub const HEART: u16 = rgb(0xff, 0x37, 0x5f);
pub const CADENCE: u16 = GREEN;
pub const GRADE: u16 = AMBER;

/// Zone colours from Z1 to Z7, shared by power and heart rate.
pub const ZONES: [u16; 7] = [
    SECONDARY,
    ACCENT,
    GREEN,
    POWER,
    AMBER,
    RED,
    rgb(0xbf, 0x5a, 0xf2),
];

/// The colour of a 1-based training zone, or TERTIARY without one.
pub const fn zone_color(zone: u8) -> u16 {
    if zone == 0 || zone as usize > ZONES.len() {
        TERTIARY
    } else {
        ZONES[zone as usize - 1]
    }
}

/// Coverage 0..=15 of a disc of `radius` around (cx, cy), all in quarter
/// pixels, at pixel (x, y). The edge is anti-aliased over one pixel.
pub const fn disc(x: usize, y: usize, cx: i32, cy: i32, radius: i32) -> u8 {
    let dx = 4 * x as i32 + 2 - cx;
    let dy = 4 * y as i32 + 2 - cy;
    let d2 = dx * dx + dy * dy;
    let inner = (radius - 2) * (radius - 2);
    let outer = (radius + 2) * (radius + 2);
    if d2 <= inner {
        15
    } else if d2 >= outer {
        0
    } else {
        (15 * (outer - d2) / (outer - inner)) as u8
    }
}

/// Coverage of a rectangle with rounded corners at (x, y).
pub const fn rounded(rect: Rect, radius: usize, x: usize, y: usize) -> u8 {
    if !rect.contains(x, y) {
        return 0;
    }
    let cx = if x < rect.left + radius {
        rect.left + radius
    } else if x >= rect.right - radius {
        rect.right - radius
    } else {
        return 15;
    };
    let cy = if y < rect.top + radius {
        rect.top + radius
    } else if y >= rect.bottom - radius {
        rect.bottom - radius
    } else {
        return 15;
    };
    disc(x, y, 4 * cx as i32, 4 * cy as i32, 4 * radius as i32)
}

/// A rounded card filled with `fill`, ringed by `ring` when focused.
pub fn card(
    rect: Rect,
    radius: usize,
    x: usize,
    y: usize,
    fill: u16,
    ring: Option<u16>,
) -> Option<u16> {
    let outer = rounded(rect, radius, x, y);
    if outer == 0 {
        return None;
    }
    let Some(ring) = ring else {
        return Some(blend(fill, BLACK, outer));
    };
    let inside = rounded(rect.inset(3), radius.saturating_sub(3), x, y);
    let edge = blend(ring, BLACK, outer);
    Some(blend(fill, edge, inside))
}

/// Paint a rounded shape whose pixels `pixel` gives. A rounded shape's row
/// is uniform between its corner columns, and so is a ring `edge` wide (a
/// focus ring) wherever it is solid, so those parts are one span; only the
/// anti-aliased edges are asked per pixel.
///
/// `pixel` must give one colour wherever the shape is solid and, with an
/// `edge`, one colour across the ring: where `rect` is solid and the same
/// shape inset by `edge` (radius less `edge`) is clear, as `card` paints it.
pub fn paint_rounded(
    row: &mut Row,
    rect: Rect,
    radius: usize,
    edge: usize,
    pixel: impl Fn(usize, usize) -> Option<u16>,
) {
    paint_rounded_over(row, rect, radius, edge, |x, y, _| pixel(x, y));
}

/// `paint_rounded` for shapes over something other than black: `pixel` is
/// also given what the row already holds there, to blend its edge over.
/// A span's colour comes from `pixel` at its first column, over what the row
/// holds there only, so solid parts must not depend on what lies beneath.
pub fn paint_rounded_over(
    row: &mut Row,
    rect: Rect,
    radius: usize,
    edge: usize,
    pixel: impl Fn(usize, usize, u16) -> Option<u16>,
) {
    let y = row.y();
    if y < rect.top || y >= rect.bottom {
        return;
    }
    let radius = radius.min(rect.width() / 2);
    let corner = y < rect.top + radius || y >= rect.bottom - radius;
    if corner && radius > 0 {
        paint_corner_row(row, rect, radius, edge, &pixel);
        return;
    }
    let side = edge.min(radius);
    let middle = rect.left + side..rect.right - side;
    if !middle.is_empty()
        && let Some(color) = pixel(middle.start, y, under(row, middle.start))
    {
        row.span(middle.clone(), color);
    }
    if side > 0 {
        for ring in [rect.left..middle.start, middle.end..rect.right] {
            if let Some(color) = pixel(ring.start, y, under(row, ring.start)) {
                row.span(ring, color);
            }
        }
    }
}

/// What the row holds at `x`: black beyond its end.
fn under(row: &Row, x: usize) -> u16 {
    row.pixels().get(x).copied().unwrap_or(BLACK)
}

/// A row through a rounded shape's corners. Each corner is a quarter of a
/// disc, and of a second one `edge` inside it when ringed; the row is the
/// middle colour wherever both are solid, empty beyond the outer one, and
/// worked out per pixel only across their anti-aliased edges.
fn paint_corner_row(
    row: &mut Row,
    rect: Rect,
    radius: usize,
    edge: usize,
    pixel: &impl Fn(usize, usize, u16) -> Option<u16>,
) {
    let y = row.y();
    let cy = if y < rect.top + radius {
        rect.top + radius
    } else {
        rect.bottom - radius
    };
    let middle = rect.left + radius..rect.right - radius;
    let color = if middle.is_empty() {
        None
    } else {
        pixel(middle.start, y, under(row, middle.start))
    };
    let discs = |cx: usize| {
        let outer = disc_row(y, 4 * cx as i32, 4 * cy as i32, 4 * radius as i32);
        let inner = (edge > 0 && edge < radius)
            .then(|| disc_row(y, 4 * cx as i32, 4 * cy as i32, 4 * (radius - edge) as i32));
        (outer, inner)
    };
    // Left corner: nothing before the outer disc, per pixel up to where
    // both discs are solid, then the middle colour.
    let ((solid, touched), inner) = discs(rect.left + radius);
    let start = touched.start.max(rect.left);
    let mut fringe_end = if solid.is_empty() {
        touched.end
    } else {
        solid.start
    };
    if let Some((solid, touched)) = &inner {
        let end = if solid.is_empty() {
            touched.end
        } else {
            solid.start
        };
        fringe_end = fringe_end.max(end);
    }
    let fringe_end = fringe_end.clamp(start, middle.start);
    match inner {
        Some((_, touched_inner)) if !solid.is_empty() && !touched_inner.is_empty() => {
            let ring = solid.start.max(start)..touched_inner.start.min(fringe_end);
            paint_ringed_fringe(row, start..fringe_end, ring, pixel);
        }
        _ => row.each_over(start..fringe_end, |x, under| pixel(x, y, under)),
    }
    // Right corner, mirrored.
    let ((solid, touched), inner) = discs(rect.right - radius);
    let end = touched.end.min(rect.right);
    let mut fringe_start = if solid.is_empty() {
        touched.start
    } else {
        solid.end
    };
    if let Some((solid, touched)) = &inner {
        let start = if solid.is_empty() {
            touched.start
        } else {
            solid.end
        };
        fringe_start = fringe_start.min(start);
    }
    let fringe_start = fringe_start.clamp(middle.end, end.max(middle.end));
    if let Some(color) = color {
        row.span(fringe_end..fringe_start, color);
    }
    match inner {
        Some((_, touched_inner)) if !solid.is_empty() && !touched_inner.is_empty() => {
            let ring = touched_inner.end.max(fringe_start)..solid.end.min(end);
            paint_ringed_fringe(row, fringe_start..end, ring, pixel);
        }
        _ => row.each_over(fringe_start..end, |x, under| pixel(x, y, under)),
    }
}

/// Paint a corner's `fringe` per pixel, but for its `ring` columns, where
/// the outer disc is solid and the inner one clear: those are one colour.
#[inline(never)]
fn paint_ringed_fringe(
    row: &mut Row,
    fringe: Range<usize>,
    ring: Range<usize>,
    pixel: &impl Fn(usize, usize, u16) -> Option<u16>,
) {
    let y = row.y();
    if ring.is_empty() {
        row.each_over(fringe, |x, under| pixel(x, y, under));
        return;
    }
    row.each_over(fringe.start..ring.start, |x, under| pixel(x, y, under));
    if let Some(color) = pixel(ring.start, y, under(row, ring.start)) {
        row.span(ring.clone(), color);
    }
    row.each_over(ring.end..fringe.end, |x, under| pixel(x, y, under));
}

/// The columns of row `y` where a `disc` of `radius` around (cx, cy), in
/// quarter pixels, is solid, and those it touches at all.
pub fn disc_row(y: usize, cx: i32, cy: i32, radius: i32) -> (Range<usize>, Range<usize>) {
    let dy = 4 * y as i32 + 2 - cy;
    // Columns whose centre offset 4x + 2 - cx lies within ±reach.
    let columns = |reach: Option<i32>| match reach {
        Some(reach) => {
            let left = (cx - reach - 2 + 3).div_euclid(4).max(0) as usize;
            let right = ((cx + reach - 2).div_euclid(4) + 1).max(0) as usize;
            left..right.max(left)
        }
        None => 0..0,
    };
    let reach = |limit: i32| (limit >= 0).then(|| (limit as u32).isqrt() as i32);
    let inner = (radius - 2) * (radius - 2) - dy * dy;
    let outer = (radius + 2) * (radius + 2) - dy * dy - 1;
    (columns(reach(inner)), columns(reach(outer)))
}

/// Paint a disc row: `solid` across its solid span and `fringe` per pixel,
/// with its coverage, at the anti-aliased edge.
pub fn paint_disc(
    row: &mut Row,
    disc: (i32, i32, i32),
    solid: u16,
    fringe: impl Fn(usize, u8) -> Option<u16>,
) {
    paint_disc_over(row, disc, solid, |x, coverage, _| fringe(x, coverage));
}

/// `paint_disc` for discs over something other than black: `fringe` is also
/// given what the row already holds there, to blend the edge over.
pub fn paint_disc_over(
    row: &mut Row,
    (cx, cy, radius): (i32, i32, i32),
    solid: u16,
    fringe: impl Fn(usize, u8, u16) -> Option<u16>,
) {
    let y = row.y();
    let (inside, touched) = disc_row(y, cx, cy, radius);
    if touched.is_empty() {
        return;
    }
    let inside = if inside.is_empty() {
        touched.start..touched.start
    } else {
        inside
    };
    row.span(inside.clone(), solid);
    let pixel = |x, under| match disc(x, y, cx, cy, radius) {
        0 => None,
        coverage => fringe(x, coverage, under),
    };
    row.each_over(touched.start..inside.start, pixel);
    row.each_over(inside.end..touched.end, pixel);
}

/// Angles are in 4096ths of a turn, clockwise from 12 o'clock.
pub const TURN: u16 = 4096;

/// Clockwise angle from 12 o'clock of the offset (`dx`, `dy`), with y
/// growing down, in `TURN`ths. Within about 0.2° of the true angle, finer
/// than a pixel on any ring that fits the screen.
pub const fn angle(dx: i32, dy: i32) -> u16 {
    let (across, up) = (dx.unsigned_abs(), dy.unsigned_abs());
    // Turned from the vertical axis towards the side the point lies on.
    let quarter = if across <= up {
        octant(across, up)
    } else {
        1024 - octant(up, across)
    } as u16;
    match (dx >= 0, dy <= 0) {
        (true, true) => quarter,
        (true, false) => 2048 - quarter,
        (false, false) => 2048 + quarter,
        (false, true) => (TURN - quarter) % TURN,
    }
}

/// atan(small / large), for small <= large, in 4096ths of a turn:
/// 512 t + 178 t (1 - t) with t in 1024ths.
const fn octant(small: u32, large: u32) -> u32 {
    if large == 0 {
        return 0;
    }
    let t = small * 1024 / large;
    (512 * t + 178 * t * (1024 - t) / 1024) / 1024
}

/// Sine of `half` half-turns, 0 to 1, by Bhaskara I's formula (within 0.2%).
fn half_sine(half: f32) -> f32 {
    let degrees = half * 180.0;
    let product = degrees * (180.0 - degrees);
    4.0 * product / (40_500.0 - product)
}

/// A point `radius` from (cx, cy) at `turn` (`TURN`ths clockwise from 12
/// o'clock), all in quarter pixels. Prepare-time only.
pub fn on_circle(cx: i32, cy: i32, radius: i32, turn: u16) -> (i32, i32) {
    let sine = |turns: f32| {
        let turns = if turns >= 1.0 { turns - 1.0 } else { turns };
        let half = turns * 2.0;
        if half < 1.0 {
            half_sine(half)
        } else {
            -half_sine(half - 1.0)
        }
    };
    let turns = f32::from(turn % TURN) / f32::from(TURN);
    let (sin, cos) = (sine(turns), sine(turns + 0.25));
    let r = radius as f32;
    ((cx as f32 + r * sin) as i32, (cy as f32 - r * cos) as i32)
}

/// An arc of a ring with round ends: VANA's shape for progress, charge and
/// countdowns. Lengths are quarter pixels; the arc runs clockwise `sweep`
/// `TURN`ths from `start`. A full sweep is a closed ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ring {
    pub cx: i32,
    pub cy: i32,
    /// The radius of the ring's centre line.
    pub radius: i32,
    pub width: i32,
    pub start: u16,
    pub sweep: u16,
}

impl Ring {
    const fn outer(&self) -> i32 {
        self.radius + self.width / 2
    }

    const fn inner(&self) -> i32 {
        self.radius - self.width / 2
    }

    /// The box the ring can cover.
    pub const fn bounds(&self) -> Rect {
        let reach = self.outer() + 2;
        Rect {
            left: ((self.cx - reach) / 4) as usize,
            top: ((self.cy - reach) / 4) as usize,
            right: ((self.cx + reach) / 4 + 1) as usize,
            bottom: ((self.cy + reach) / 4 + 1) as usize,
        }
    }

    /// The round ends' centres, in quarter pixels.
    pub fn ends(&self) -> [(i32, i32); 2] {
        let end = (u32::from(self.start) + u32::from(self.sweep)) % u32::from(TURN);
        [
            on_circle(self.cx, self.cy, self.radius, self.start),
            on_circle(self.cx, self.cy, self.radius, end as u16),
        ]
    }

    /// Coverage 0..=15 at pixel (x, y); `ends` from `Ring::ends`.
    pub fn coverage(&self, x: usize, y: usize, ends: &[(i32, i32); 2]) -> u8 {
        if self.sweep == 0 {
            return 0;
        }
        let band = disc(x, y, self.cx, self.cy, self.outer())
            .min(15 - disc(x, y, self.cx, self.cy, self.inner()));
        if band == 0 || self.sweep >= TURN {
            return band;
        }
        let dx = 4 * x as i32 + 2 - self.cx;
        let dy = 4 * y as i32 + 2 - self.cy;
        let along = (angle(dx, dy) + TURN - self.start % TURN) % TURN;
        if along < self.sweep {
            return band;
        }
        let cap = self.width / 2;
        let [a, b] = *ends;
        disc(x, y, a.0, a.1, cap).max(disc(x, y, b.0, b.1, cap))
    }

    /// Paint the ring's row in `color` over what the row holds. Only the
    /// band between its two circles is worked out, a pixel at a time.
    pub fn paint(&self, row: &mut Row, ends: &[(i32, i32); 2], color: u16) {
        if self.sweep == 0 {
            return;
        }
        let y = row.y();
        let (_, touched) = disc_row(y, self.cx, self.cy, self.outer());
        if touched.is_empty() {
            return;
        }
        let (hole, _) = disc_row(y, self.cx, self.cy, self.inner() - 4);
        let pixel = |x: usize, under: u16| match self.coverage(x, y, ends) {
            0 => None,
            coverage => Some(blend(color, under, coverage)),
        };
        if hole.is_empty() {
            row.each_over(touched, pixel);
        } else {
            row.each_over(touched.start..hole.start, pixel);
            row.each_over(hole.end..touched.end, pixel);
        }
    }
}

/// The ride state the status bar marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    None,
    /// A red dot and REC.
    Recording,
    /// Amber pause bars and PAUSED.
    Paused,
    /// Amber pause bars and AUTO; the toast spells out AUTO PAUSED.
    AutoPaused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gps {
    #[default]
    Off,
    Searching,
    Fixed,
}

/// Page dots follow the ride state on the left of the status bar.
pub const DOTS_Y: usize = 11;
pub const DOT_SIZE: usize = 6;
pub const DOT_PITCH: usize = 11;
/// The pitch dots close up to when the bar is crowded.
const TIGHT_DOT_PITCH: usize = 8;
/// One clear column between dots: the last resort before GPS gives way.
const TIGHTEST_DOT_PITCH: usize = 7;

const MIDDLE: usize = STATUS_BAR.bottom / 2;
const EDGE: usize = 8;
/// The mark's dot or pause bars, before its word.
const MARK_ICON: usize = 10;
/// Battery icon: a 19 × 10 pill with a nub.
const BATTERY_WIDTH: usize = 19;
/// Below this the percentage shows in place of the icon.
pub const LOW_BATTERY: u8 = 20;
/// At or below this the battery's fill turns amber.
pub const ATTENTION_BATTERY: u8 = 30;
/// The charging bolt left of the icon, and the space after it.
const BOLT_WIDTH: usize = 7;
const BOLT_GAP: usize = 2;
/// A charging battery's fill climbs from its level to full in this many
/// frames, then starts again.
pub const CHARGE_STEPS: u8 = 4;

/// The charging bolt, 7 × 11, as 4-bit coverage: `#` solid, `+` half.
const BOLT: [&[u8; BOLT_WIDTH]; 11] = [
    b"...+##.", b"...##+.", b"..+##..", b"..##+..", b".+#####", b".####+.", b"...##+.", b"..+##..",
    b"..##+..", b".+#+...", b".#+....",
];

/// Coverage of the bolt whose top-left is (`left`, `top`) at (x, y).
pub const fn bolt(left: usize, top: usize, x: usize, y: usize) -> u8 {
    if x < left || y < top || x >= left + BOLT_WIDTH || y >= top + BOLT.len() {
        return 0;
    }
    match BOLT[y - top][x - left] {
        b'#' => 15,
        b'+' => 8,
        _ => 0,
    }
}

/// What the status bar knows about the battery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Battery {
    /// Charge, 0 to 100, as the device reports it.
    pub percent: u8,
    /// USB power is connected and the device reports charging.
    pub charging: bool,
    /// The charging animation's frame, 0 to `CHARGE_STEPS - 1`.
    pub phase: u8,
}

impl Battery {
    pub const fn level(percent: u8) -> Self {
        Self {
            percent,
            charging: false,
            phase: 0,
        }
    }

    /// The share of the icon to fill, 0 to 100: the level, or while
    /// charging a fill that climbs from the level to full and starts again.
    pub const fn fill(self) -> u8 {
        let level = if self.percent > 100 {
            100
        } else {
            self.percent
        };
        if !self.charging {
            return level;
        }
        let step = (self.phase % CHARGE_STEPS) as u16;
        let rise = (100 - level as u16) * step / (CHARGE_STEPS as u16 - 1);
        (level as u16 + rise) as u8
    }

    /// The fill's colour: green while charging, red once low, amber at 30%
    /// or less, white otherwise.
    pub const fn color(self) -> u16 {
        if self.charging {
            GREEN
        } else if self.percent < LOW_BATTERY {
            RED
        } else if self.percent <= ATTENTION_BATTERY {
            AMBER
        } else {
            WHITE
        }
    }
}
/// The least space between two status bar items.
const GAP: usize = 6;
/// Anti-aliased text may cover this many columns past its advance width.
const SPILL: usize = 2;

type Small = Label<8>;

const fn small(text: &[u8], x: usize, align: Align, color: u16) -> Small {
    let line = Line::new(&CAPTION, text);
    let top = MIDDLE - CAPTION.top as usize - CAPTION.cap as usize / 2;
    Label::new(line, x, top, align, color)
}

/// What the status bar shows, prepared once per frame.
///
/// Items are laid out with measured widths in priority order: the ride mark,
/// the clock, the battery (a red percentage once low), GPS, then page dots,
/// which close up before they are dropped. An item that does not fit is left
/// out; nothing overlaps.
#[derive(Clone, Copy)]
pub struct StatusBar {
    clock: Small,
    /// The battery, when its icon fits.
    battery: Option<Battery>,
    battery_left: usize,
    percent: Option<Small>,
    gps: Option<Small>,
    mark: Mark,
    mark_label: Small,
    /// The first column of the right-hand items.
    right_limit: usize,
    /// The same without the GPS label, which gives way to page dots.
    limit_without_gps: usize,
    /// Current page and page count; no dots below two pages.
    pages: (usize, usize),
    /// Left edge and pitch of the page dots, when they fit.
    dots: Option<(usize, usize)>,
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new(&Text::new(), Gps::Off, None, Mark::None)
    }
}

impl StatusBar {
    pub fn new(clock: &Text<16>, gps: Gps, battery: Option<u8>, mark: Mark) -> Self {
        Self::with_battery(clock, gps, battery.map(Battery::level), mark)
    }

    /// The bar with a battery that may be charging: a green bolt then
    /// shows left of the icon, and the icon stays even when low.
    pub fn with_battery(clock: &Text<16>, gps: Gps, battery: Option<Battery>, mark: Mark) -> Self {
        let line = Line::new(&BODY, clock.as_bytes());
        let top = MIDDLE - BODY.top as usize - BODY.cap as usize / 2;
        let clock = Label::new(line, WIDTH - EDGE, top, Align::Right, WHITE);
        let (word, color): (&[u8], u16) = match mark {
            Mark::None => (b"", BLACK),
            Mark::Recording => (b"REC", RED),
            Mark::Paused => (b"PAUSED", AMBER),
            Mark::AutoPaused => (b"AUTO", AMBER),
        };
        let mut mark_label = small(word, EDGE + MARK_ICON + 4, Align::Left, color);
        if mark_label.right() + SPILL + GAP > clock.left {
            mark_label = small(b"", EDGE + MARK_ICON + 4, Align::Left, color);
        }
        let mut bar = Self {
            clock,
            battery: None,
            battery_left: 0,
            percent: None,
            gps: None,
            mark,
            mark_label,
            right_limit: clock.left,
            limit_without_gps: clock.left,
            pages: (0, 0),
            dots: None,
        };
        let mark_end = bar.mark_end();
        let fits = |left: usize| left >= mark_end + GAP;
        let battery_left = clock.left.saturating_sub(GAP + BATTERY_WIDTH);
        let bolt_left = battery_left.saturating_sub(BOLT_GAP + BOLT_WIDTH);
        match battery {
            // A low battery shows as its red percentage in place of the icon.
            Some(battery) if !battery.charging && battery.percent < LOW_BATTERY => {
                let level = battery.percent;
                let mut text = Text::<4>::new();
                let _ = core::fmt::Write::write_fmt(&mut text, format_args!("{level}%"));
                let label = small(text.as_bytes(), clock.left - GAP, Align::Right, RED);
                if fits(label.left) {
                    bar.right_limit = label.left;
                    bar.percent = Some(label);
                }
            }
            Some(battery) if battery.charging && fits(bolt_left) => {
                bar.battery = Some(battery);
                bar.battery_left = battery_left;
                bar.right_limit = bolt_left;
            }
            Some(battery) if fits(battery_left) => {
                bar.battery = Some(Battery {
                    charging: false,
                    ..battery
                });
                bar.battery_left = battery_left;
                bar.right_limit = battery_left;
            }
            _ => {}
        }
        let gps_color = match gps {
            Gps::Off => None,
            Gps::Searching => Some(TERTIARY),
            Gps::Fixed => Some(GREEN),
        };
        bar.limit_without_gps = bar.right_limit;
        if let Some(color) = gps_color {
            let label = small(b"GPS", bar.right_limit - 6, Align::Right, color);
            if fits(label.left) {
                bar.right_limit = label.left;
                bar.gps = Some(label);
            }
        }
        bar
    }

    /// The column after the mark, or 0 with no mark.
    fn mark_end(&self) -> usize {
        match self.mark {
            Mark::None => 0,
            _ if self.mark_label.line.width() == 0 => EDGE + MARK_ICON,
            _ => self.mark_label.right() + SPILL,
        }
    }

    pub fn set_pages(&mut self, index: usize, count: usize) {
        self.pages = (index, count);
        let left = match self.mark {
            Mark::None => EDGE,
            _ => self.mark_end() + GAP,
        };
        let pitch = |limit: usize| {
            [DOT_PITCH, TIGHT_DOT_PITCH, TIGHTEST_DOT_PITCH]
                .into_iter()
                .find(|pitch| left + (count - 1) * pitch + DOT_SIZE + GAP <= limit)
        };
        self.dots = None;
        if count > 1 {
            let mut found = pitch(self.right_limit);
            if found.is_none() && self.gps.is_some() {
                found = pitch(self.limit_without_gps);
                if found.is_some() {
                    self.gps = None;
                }
            }
            self.dots = found.map(|pitch| (left, pitch));
        }
    }

    /// The columns the page dots cover, empty when they are not shown.
    fn dot_columns(&self) -> Range<usize> {
        match self.dots {
            Some((left, pitch)) => left..left + (self.pages.1 - 1) * pitch + DOT_SIZE,
            None => 0..0,
        }
    }

    /// Paint a status bar row: black, then each item in its own columns.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        row.fill(BLACK);
        // The mark, battery and dots are drawn about the middle row.
        let shapes = (MIDDLE - 6..MIDDLE + 6).contains(&y);
        self.mark_label.paint(row);
        if shapes && self.mark != Mark::None {
            row.each(EDGE..EDGE + MARK_ICON, |x| Some(self.icon_pixel(x, y)));
        }
        for label in [&self.percent, &self.gps].into_iter().flatten() {
            label.paint(row);
        }
        if shapes && self.battery.is_some() {
            row.each(self.battery_columns(), |x| Some(self.battery_pixel(x, y)));
        }
        self.clock.paint(row);
        if shapes {
            row.each(self.dot_columns(), |x| Some(self.dots_pixel(x, y)));
        }
    }

    /// Pixels for y below `CONTENT.top`.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let text = |label: &Small| {
            (label.left..label.right() + SPILL)
                .contains(&x)
                .then(|| label.pixel(x, y, BLACK).unwrap_or(BLACK))
        };
        if let Some(pixel) = text(&self.clock) {
            return pixel;
        }
        if self.battery.is_some() && self.battery_columns().contains(&x) {
            return self.battery_pixel(x, y);
        }
        for label in [&self.percent, &self.gps].into_iter().flatten() {
            if let Some(pixel) = text(label) {
                return pixel;
            }
        }
        if self.dot_columns().contains(&x) {
            return self.dots_pixel(x, y);
        }
        if self.mark != Mark::None && (EDGE..EDGE + MARK_ICON).contains(&x) {
            return self.icon_pixel(x, y);
        }
        text(&self.mark_label).unwrap_or(BLACK)
    }

    /// The columns each shown item covers, left to right, for layout checks.
    #[cfg(test)]
    pub(crate) fn items(&self) -> std::vec::Vec<(&'static str, Range<usize>)> {
        let text = |label: &Small| label.left..label.right() + SPILL;
        let mut items = std::vec::Vec::new();
        if self.mark != Mark::None {
            items.push(("mark", EDGE..self.mark_end()));
        }
        if self.dots.is_some() {
            items.push(("dots", self.dot_columns()));
        }
        if let Some(gps) = &self.gps {
            items.push(("gps", text(gps)));
        }
        if let Some(percent) = &self.percent {
            items.push(("percent", text(percent)));
        }
        if self.battery.is_some() {
            items.push(("battery", self.battery_columns()));
        }
        items.push(("clock", text(&self.clock)));
        items
    }

    fn icon_pixel(&self, x: usize, y: usize) -> u16 {
        let coverage = match self.mark {
            Mark::None => 0,
            // A dot of radius 4.5 centred in a 10 × 10 cell.
            Mark::Recording => disc(x, y, 4 * EDGE as i32 + 18, 4 * MIDDLE as i32, 18),
            Mark::Paused | Mark::AutoPaused => {
                let bar = |left: usize| {
                    let rect = Rect {
                        left,
                        top: MIDDLE - 5,
                        right: left + 3,
                        bottom: MIDDLE + 5,
                    };
                    rounded(rect, 1, x, y)
                };
                bar(EDGE + 1).max(bar(EDGE + 6))
            }
        };
        blend(self.mark_label.color, BLACK, coverage)
    }

    /// The battery icon's columns, its bolt included while charging.
    fn battery_columns(&self) -> Range<usize> {
        let charging = self.battery.is_some_and(|b| b.charging);
        let bolt = if charging { BOLT_GAP + BOLT_WIDTH } else { 0 };
        self.battery_left - bolt..self.battery_left + BATTERY_WIDTH
    }

    fn battery_pixel(&self, x: usize, y: usize) -> u16 {
        let battery = self.battery.unwrap_or_default();
        let left = self.battery_left;
        if x < left {
            let top = MIDDLE - BOLT.len() / 2;
            let coverage = bolt(left - BOLT_GAP - BOLT_WIDTH, top, x, y);
            return blend(GREEN, BLACK, coverage);
        }
        let percent = usize::from(battery.fill());
        let body = Rect {
            left,
            top: MIDDLE - 5,
            right: left + BATTERY_WIDTH - 2,
            bottom: MIDDLE + 5,
        };
        let nub = Rect {
            left: body.right + 1,
            top: MIDDLE - 2,
            right: body.right + 2,
            bottom: MIDDLE + 2,
        };
        if nub.contains(x, y) {
            return SECONDARY;
        }
        let color = battery.color();
        let fill = Rect {
            left: body.left + 2,
            top: body.top + 2,
            right: body.left + 2 + (percent * (body.width() - 4)).div_ceil(100),
            bottom: body.bottom - 2,
        };
        if fill.contains(x, y) {
            return color;
        }
        let shell = rounded(body, 3, x, y).saturating_sub(rounded(body.inset(1), 2, x, y));
        blend(SECONDARY, BLACK, shell)
    }

    fn dots_pixel(&self, x: usize, y: usize) -> u16 {
        let (index, count) = self.pages;
        let Some((left, pitch)) = self.dots else {
            return BLACK;
        };
        if x < left || (x - left) / pitch >= count {
            return BLACK;
        }
        let dot = (x - left) / pitch;
        let cx = 4 * (left + dot * pitch) as i32 + 2 * DOT_SIZE as i32;
        let coverage = disc(x, y, cx, 4 * DOTS_Y as i32, 2 * DOT_SIZE as i32 - 2);
        blend(if dot == index { WHITE } else { TERTIARY }, BLACK, coverage)
    }
}

/// How a toast should read: the colour of its dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Confirmations: saved, resumed.
    Good,
    /// Hints and neutral news: a lap, a page.
    Info,
    /// A press is armed or a state needs attention.
    Warn,
    /// Something failed.
    Alert,
}

impl Tone {
    pub const fn color(self) -> u16 {
        match self {
            Self::Good => GREEN,
            Self::Info => ACCENT,
            Self::Warn => AMBER,
            Self::Alert => RED,
        }
    }
}

const TOAST_HEIGHT: usize = 30;
const TOAST_DOT: usize = 8;
/// Pill padding: before the dot, between dot and text, after the text.
const TOAST_PADDING: usize = 12 + 6 + 14;

/// A short message in a pill floating over the bottom of the content. The
/// owner decides how long it lives; above its band it costs one comparison.
#[derive(Clone, Copy)]
pub struct Toast {
    label: Label<20>,
    pill: Rect,
    pub tone: Tone,
    /// Long messages drop the tone dot to fit.
    dot: bool,
}

impl Toast {
    /// Text that does not fit the screen is cut.
    pub fn new(message: &str, tone: Tone) -> Self {
        let bytes = message.as_bytes();
        let mut len = bytes.len().min(20);
        while len > 0 && BODY.width(&bytes[..len]) + 24 > WIDTH - 4 {
            len -= 1;
        }
        let line = Line::<20>::new(&BODY, &bytes[..len]);
        let dot = line.width() + TOAST_DOT + TOAST_PADDING <= WIDTH - 4;
        let width = if dot {
            line.width() + TOAST_DOT + TOAST_PADDING
        } else {
            line.width() + 24
        };
        let top = TOAST.bottom - 8 - TOAST_HEIGHT;
        let pill = Rect {
            left: (WIDTH - width) / 2,
            top,
            right: (WIDTH - width) / 2 + width,
            bottom: top + TOAST_HEIGHT,
        };
        let text_top = top + TOAST_HEIGHT / 2 - BODY.top as usize - BODY.cap as usize / 2;
        let left = pill.left + if dot { 12 + TOAST_DOT + 6 } else { 12 };
        let label = Label::new(line, left, text_top, Align::Left, WHITE);
        Self {
            label,
            pill,
            tone,
            dot,
        }
    }

    pub fn text(&self) -> &[u8] {
        self.label.line.text()
    }

    /// Paint the pill over the row: its edge per pixel, the tone dot, then
    /// the text over the solid middle.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        if y < self.pill.top || y >= self.pill.bottom {
            return;
        }
        let radius = TOAST_HEIGHT / 2;
        paint_rounded(row, self.pill, radius, 0, |x, y| {
            match rounded(self.pill, radius, x, y) {
                0 => None,
                coverage => Some(blend(RAISED, BLACK, coverage)),
            }
        });
        if self.dot {
            let cx = 4 * (self.pill.left + 12) as i32 + 2 * TOAST_DOT as i32;
            let cy = 4 * (self.pill.top + TOAST_HEIGHT / 2) as i32;
            let color = self.tone.color();
            paint_disc(row, (cx, cy, 2 * TOAST_DOT as i32), color, |_, dot| {
                Some(blend(color, RAISED, dot))
            });
        }
        self.label.paint(row);
    }

    /// The toast's pixel, or none where content shows through.
    pub fn pixel(&self, x: usize, y: usize) -> Option<u16> {
        if y < self.pill.top {
            return None;
        }
        let coverage = rounded(self.pill, TOAST_HEIGHT / 2, x, y);
        if coverage == 0 {
            return None;
        }
        if coverage < 15 {
            return Some(blend(RAISED, BLACK, coverage));
        }
        let cx = 4 * (self.pill.left + 12) as i32 + 2 * TOAST_DOT as i32;
        let cy = 4 * (self.pill.top + TOAST_HEIGHT / 2) as i32;
        let dot = if self.dot {
            disc(x, y, cx, cy, 2 * TOAST_DOT as i32)
        } else {
            0
        };
        if dot > 0 {
            return Some(blend(self.tone.color(), RAISED, dot));
        }
        Some(self.label.pixel(x, y, RAISED).unwrap_or(RAISED))
    }
}

/// Checks for the layout sweeps: where text really inks, and that nothing
/// on a screen overlaps or leaves its region.
#[cfg(test)]
pub(crate) mod layout {
    use super::{Label, Rect};
    use std::string::String;
    use std::vec::Vec;

    /// The box a label inks, or `None` for blank text.
    pub(crate) fn ink<const N: usize>(label: &Label<N>) -> Option<Rect> {
        let mut ink: Option<Rect> = None;
        for y in label.top..label.bottom() {
            for x in label.columns() {
                if label.line.coverage(x - label.left, y - label.top) == 0 {
                    continue;
                }
                let r = ink.get_or_insert(Rect {
                    left: x,
                    top: y,
                    right: x + 1,
                    bottom: y + 1,
                });
                r.left = r.left.min(x);
                r.top = r.top.min(y);
                r.right = r.right.max(x + 1);
                r.bottom = r.bottom.max(y + 1);
            }
        }
        ink
    }

    /// A named item for `assert_apart`: a label's ink, or nothing if blank.
    pub(crate) fn text<const N: usize>(label: &Label<N>) -> Option<(String, Rect)> {
        let name = String::from_utf8_lossy(label.line.text()).into_owned();
        ink(label).map(|rect| (name, rect))
    }

    /// Clear columns between two items on the same line: less reads as one
    /// word.
    pub(crate) const WORD_GAP: usize = 4;

    /// Whether two inks overlap, or sit on one line closer than `WORD_GAP`.
    const fn meet(a: Rect, b: Rect) -> bool {
        a.left < b.right + WORD_GAP
            && b.left < a.right + WORD_GAP
            && a.top < b.bottom
            && b.top < a.bottom
    }

    /// No two items overlap or run together on a line, and every item lies
    /// inside `bounds`.
    #[track_caller]
    pub(crate) fn assert_apart(items: &[(String, Rect)], bounds: Rect, what: &str) {
        for (i, (name, rect)) in items.iter().enumerate() {
            let inside = rect.left >= bounds.left
                && rect.top >= bounds.top
                && rect.right <= bounds.right
                && rect.bottom <= bounds.bottom;
            assert!(inside, "{what}: {name:?} {rect:?} leaves {bounds:?}");
            for (other, against) in &items[i + 1..] {
                assert!(
                    !meet(*rect, *against),
                    "{what}: {name:?} {rect:?} overlaps or touches {other:?} {against:?}"
                );
            }
        }
    }

    /// Every text in `want` shows whole as one of the items.
    #[track_caller]
    pub(crate) fn assert_shown(items: &[(String, Rect)], want: &[&str], what: &str) {
        let names: Vec<&str> = items.iter().map(|(name, _)| name.as_str()).collect();
        for text in want {
            assert!(
                names.contains(text),
                "{what}: {text:?} not whole in {names:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(value: &str) -> Text<16> {
        let mut text = Text::new();
        let _ = text.push_str(value);
        text
    }

    fn lit(bar: &StatusBar, color: u16, xs: core::ops::Range<usize>) -> bool {
        (0..STATUS_BAR.bottom).any(|y| xs.clone().any(|x| bar.pixel(x, y) == color))
    }

    #[test]
    fn layout_derives_from_one_status_bar() {
        assert_eq!(CONTENT.top, 24);
        const { assert!(TOAST.top > CONTENT.top && TOAST.bottom == CONTENT.bottom) };
        assert_eq!(rgb(0xff, 0xff, 0xff), WHITE);
    }

    #[test]
    fn shapes_are_solid_inside_and_soft_at_the_edge() {
        let rect = Rect {
            left: 10,
            top: 10,
            right: 50,
            bottom: 30,
        };
        assert_eq!(rounded(rect, 6, 30, 20), 15);
        assert_eq!(rounded(rect, 6, 10, 10), 0, "the corner is cut");
        assert_eq!(rounded(rect, 6, 9, 20), 0);
        let edge = rounded(rect, 6, 11, 12);
        assert!(edge > 0 && edge < 15, "{edge}");
        assert_eq!(disc(20, 20, 82, 82, 16), 15);
        assert_eq!(disc(30, 20, 82, 82, 16), 0);
        assert_eq!(card(rect, 6, 30, 10, CARD, Some(ACCENT)), Some(ACCENT));
        assert_eq!(card(rect, 6, 30, 20, CARD, Some(ACCENT)), Some(CARD));
        assert_eq!(card(rect, 6, 5, 20, CARD, None), None);
    }

    #[test]
    fn status_bar_marks_ride_state_gps_battery_and_pages() {
        let mut bar = StatusBar::new(&clock("9:41"), Gps::Fixed, Some(12), Mark::Recording);
        bar.set_pages(1, 3);
        assert!(lit(&bar, RED, 0..20), "red REC dot");
        assert!(lit(&bar, RED, 20..60), "REC");
        assert!(lit(&bar, GREEN, 80..200), "fixed GPS");
        assert!(lit(&bar, WHITE, 200..240), "clock");
        // The second of three dots is lit.
        let (left, pitch) = bar.dots.expect("three dots fit");
        let centre = |i: usize| left + i * pitch + DOT_SIZE / 2;
        let dots: [u16; 3] = core::array::from_fn(|i| bar.pixel(centre(i), DOTS_Y));
        assert_eq!(dots, [TERTIARY, WHITE, TERTIARY]);
        assert!(bar.percent.is_some(), "low battery shows its percentage");

        let paused = StatusBar::new(&clock("12:59"), Gps::Searching, Some(80), Mark::Paused);
        assert!(lit(&paused, AMBER, 0..70));
        assert!(!lit(&paused, RED, 0..240));
        assert!(paused.percent.is_none());
    }

    #[test]
    fn a_charging_battery_shows_a_bolt_and_a_climbing_fill() {
        let charging = |percent, phase| {
            StatusBar::with_battery(
                &clock("9:41"),
                Gps::Fixed,
                Some(Battery {
                    percent,
                    charging: true,
                    phase,
                }),
                Mark::Recording,
            )
        };
        // Even a low battery keeps its icon while charging, in green.
        let low = charging(12, 0);
        assert!(low.percent.is_none() && low.battery.is_some());
        check_status_bar(&low, "charging 12%");
        let green = |bar: &StatusBar| {
            (0..STATUS_BAR.bottom)
                .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
                .filter(|&(x, y)| bar.pixel(x, y) == GREEN)
                .count()
        };
        let first = green(&low);
        let last = green(&charging(12, CHARGE_STEPS - 1));
        assert!(last > first + 20, "the fill climbs: {first} then {last}");
        assert!(!lit(&low, RED, 140..240), "no red while charging");
        assert_eq!(Battery::level(64).fill(), 64);
        let full = Battery {
            percent: 100,
            charging: true,
            phase: 1,
        };
        assert_eq!(full.fill(), 100);
    }

    #[test]
    fn angles_run_clockwise_from_twelve_and_rings_paint_their_reference() {
        assert_eq!(angle(0, -10), 0);
        assert_eq!(angle(10, 0), 1024);
        assert_eq!(angle(0, 10), 2048);
        assert_eq!(angle(-10, 0), 3072);
        let diagonal = angle(10, -10);
        assert!(diagonal.abs_diff(512) <= 2, "{diagonal}");
        let (x, y) = on_circle(0, 0, 400, 1024);
        assert!((x - 400).abs() <= 2 && y.abs() <= 2, "{x},{y}");
        let ring = Ring {
            cx: 4 * 60,
            cy: 4 * 60,
            radius: 4 * 40,
            width: 4 * 8,
            start: 512,
            sweep: 1024,
        };
        let ends = ring.ends();
        const UNDER: u16 = 0x0841;
        let mut lit = 0;
        for y in 0..120 {
            let mut pixels = [UNDER; 120];
            ring.paint(&mut Row::new(y, &mut pixels), &ends, WHITE);
            for (x, &pixel) in pixels.iter().enumerate() {
                let coverage = ring.coverage(x, y, &ends);
                let want = if coverage == 0 {
                    UNDER
                } else {
                    blend(WHITE, UNDER, coverage)
                };
                assert_eq!(pixel, want, "{x},{y}");
                lit += usize::from(pixel == WHITE);
            }
        }
        // A quarter of the band, from 1:30 to 4:30, with round ends.
        assert!(ring.coverage(60 + 40, 60, &ends) == 15, "3 o'clock");
        assert_eq!(ring.coverage(60, 20, &ends), 0, "12 o'clock");
        assert_eq!(ring.coverage(60, 100, &ends), 0, "6 o'clock");
        assert!((300..700).contains(&lit), "{lit}");
    }

    #[test]
    fn status_bar_items_never_overlap_or_clip() {
        let marks = [Mark::None, Mark::Recording, Mark::Paused, Mark::AutoPaused];
        let gps = [Gps::Off, Gps::Searching, Gps::Fixed];
        let batteries = [
            None,
            Some(100),
            Some(80),
            Some(25),
            Some(19),
            Some(12),
            Some(5),
        ];
        let clocks = ["--:--", "9:41", "12:59", "23:59", "10:00"];
        for mark in marks {
            for gps in gps {
                for battery in batteries {
                    for clock_text in clocks {
                        for pages in [0_usize, 1, 2, 3, 4, 5, 6, 7] {
                            let mut bar = StatusBar::new(&clock(clock_text), gps, battery, mark);
                            bar.set_pages(pages.saturating_sub(1), pages);
                            let what = std::format!(
                                "{mark:?} {gps:?} {battery:?} {clock_text} {pages} pages"
                            );
                            check_status_bar(&bar, &what);
                            // The ride mark and the clock always show in full.
                            let names: std::vec::Vec<_> =
                                bar.items().iter().map(|(name, _)| *name).collect();
                            assert!(names.contains(&"clock"), "{what}");
                            if mark != Mark::None {
                                assert!(bar.mark_label.line.width() > 0, "{what}: mark word");
                            }
                            // The page dots always show; GPS gives way to them.
                            if (2..=7).contains(&pages) {
                                assert!(names.contains(&"dots"), "{what}: {:?}", bar.items());
                            }
                            if battery.is_some() {
                                let shown =
                                    names.contains(&"battery") || names.contains(&"percent");
                                assert!(shown, "{what}: {names:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// Items sit in their own columns with room between them, inside the
    /// screen, and every lit pixel belongs to one of them.
    #[track_caller]
    fn check_status_bar(bar: &StatusBar, what: &str) {
        let items = bar.items();
        for pair in items.windows(2) {
            let ((a, left), (b, right)) = (&pair[0], &pair[1]);
            assert!(
                left.end + 4 <= right.start,
                "{what}: {a} {left:?} meets {b} {right:?}"
            );
        }
        assert!(
            items.iter().all(|(_, columns)| columns.end <= WIDTH),
            "{what}"
        );
        for y in 0..STATUS_BAR.bottom {
            let mut pixels = [0x1234; WIDTH];
            bar.paint(&mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, bar.pixel(x, y), "{what}: paint at {x},{y}");
                if pixel != BLACK {
                    assert!(
                        items.iter().any(|(_, columns)| columns.contains(&x)),
                        "{what}: stray pixel at {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn toast_floats_in_its_band_and_fits_the_screen() {
        let toast = Toast::new("PRESS AGAIN TO SAVE", Tone::Warn);
        assert_eq!(toast.text(), b"PRESS AGAIN TO SAVE");
        assert!(toast.pill.left > 0 && toast.pill.right < WIDTH);
        assert!(toast.pill.top >= TOAST.top);
        assert_eq!(toast.pixel(120, TOAST.top - 1), None);
        let short = Toast::new("PAUSED", Tone::Warn);
        let dot = (short.pill.left + 16, short.pill.top + 15);
        assert_eq!(short.pixel(dot.0, dot.1), Some(AMBER));
        assert_eq!(
            short.pixel(2, short.pill.top + 15),
            None,
            "content beside it"
        );
        let long = Toast::new("A MESSAGE THAT IS FAR TOO LONG", Tone::Info);
        assert!(long.pill.right <= WIDTH && long.text().len() < 20);
    }

    #[test]
    fn painted_rows_match_the_per_pixel_chrome() {
        for (clock_text, gps, battery, mark, pages) in [
            ("9:41", Gps::Fixed, Some(12), Mark::Recording, (1, 4)),
            ("12:59", Gps::Searching, Some(80), Mark::Paused, (3, 4)),
            ("0:00", Gps::Off, None, Mark::None, (0, 0)),
            ("23:59", Gps::Fixed, Some(25), Mark::None, (0, 3)),
        ] {
            let mut bar = StatusBar::new(&clock(clock_text), gps, battery, mark);
            bar.set_pages(pages.0, pages.1);
            for y in 0..STATUS_BAR.bottom {
                let mut pixels = [0x1234; WIDTH];
                bar.paint(&mut Row::new(y, &mut pixels));
                let want: [u16; WIDTH] = core::array::from_fn(|x| bar.pixel(x, y));
                assert_eq!(pixels, want, "{clock_text} row {y}");
            }
        }
        const UNDER: u16 = 0x0841;
        for (message, tone) in [
            ("PAUSED", Tone::Warn),
            ("RIDE SAVED", Tone::Good),
            ("A MESSAGE THAT IS FAR TOO LONG", Tone::Info),
        ] {
            let toast = Toast::new(message, tone);
            for y in TOAST.top..HEIGHT {
                let mut pixels = [UNDER; WIDTH];
                toast.paint(&mut Row::new(y, &mut pixels));
                let want: [u16; WIDTH] =
                    core::array::from_fn(|x| toast.pixel(x, y).unwrap_or(UNDER));
                assert_eq!(pixels, want, "{message} row {y}");
            }
        }
        let rect = Rect {
            left: 10,
            top: 30,
            right: 230,
            bottom: 90,
        };
        for radius in [3, 4, 7, 12, 16, 18, 29] {
            for ring in [None, Some(ACCENT)] {
                for y in 25..95 {
                    let mut pixels = [UNDER; WIDTH];
                    let edge = if ring.is_some() { 3 } else { 0 };
                    paint_rounded(&mut Row::new(y, &mut pixels), rect, radius, edge, |x, y| {
                        card(rect, radius, x, y, RAISED, ring)
                    });
                    let want: [u16; WIDTH] = core::array::from_fn(|x| {
                        card(rect, radius, x, y, RAISED, ring).unwrap_or(UNDER)
                    });
                    assert_eq!(pixels, want, "card of radius {radius}, row {y}");
                }
            }
        }
    }

    #[test]
    fn zones_have_colours_and_unknown_is_dim() {
        assert_eq!(zone_color(0), TERTIARY);
        assert_eq!(zone_color(5), AMBER);
        assert_eq!(zone_color(8), TERTIARY);
    }
}
