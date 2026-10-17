//! Button cues: a quarter disc in the corner beside a physical button, with
//! an icon of what that button does on this screen.
//!
//! Cues appear only on screens where the rider decides something about the
//! ride (ready, paused, end ride, ride saved), never on live ride pages and
//! never as words. Their colour follows the palette: `GREEN` disc for the
//! action that moves forward (start, resume, save, done), a `RAISED` disc for
//! stepping back or ending. See `docs/ui.md`, "Button cues".
use super::chrome::{BLACK, GREEN, HEIGHT, RAISED, Row, WHITE, blend, disc, paint_disc_over};

/// Which button a cue sits beside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// BACK / MARK.
    TopLeft,
    /// ACT.
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Start or resume the ride.
    Play,
    /// End the ride.
    Stop,
    /// Confirm or finish.
    Check,
    /// Go back without changing anything.
    Back,
}

/// The visible quarter disc's radius.
pub const RADIUS: usize = 42;
/// The icon's centre, this far in from both edges of its corner.
const ICON_INSET: i32 = 17;
/// The box around the icon centre that the icon can touch, in pixels.
const ICON_REACH: i32 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    pub corner: Corner,
    pub icon: Icon,
}

impl Cue {
    pub const fn new(corner: Corner, icon: Icon) -> Self {
        Self { corner, icon }
    }

    /// Forward actions are green with a black icon; others are raised grey
    /// with a white one.
    pub const fn colors(&self) -> (u16, u16) {
        match self.icon {
            Icon::Play | Icon::Check => (GREEN, BLACK),
            Icon::Stop | Icon::Back => (RAISED, WHITE),
        }
    }

    /// The disc's centre: the screen corner, in pixels.
    const fn corner_xy(&self) -> (i32, i32) {
        match self.corner {
            Corner::TopLeft => (0, 0),
            Corner::BottomRight => (super::chrome::WIDTH as i32, HEIGHT as i32),
        }
    }

    const fn icon_xy(&self) -> (i32, i32) {
        match self.corner {
            Corner::TopLeft => (ICON_INSET, ICON_INSET),
            Corner::BottomRight => (
                super::chrome::WIDTH as i32 - ICON_INSET,
                HEIGHT as i32 - ICON_INSET,
            ),
        }
    }

    /// Rows the cue covers.
    pub const fn rows(&self) -> core::ops::Range<usize> {
        match self.corner {
            Corner::TopLeft => 0..RADIUS + 1,
            Corner::BottomRight => HEIGHT - RADIUS - 1..HEIGHT,
        }
    }

    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        if !self.rows().contains(&y) {
            return;
        }
        let (fill, ink) = self.colors();
        let (cx, cy) = self.corner_xy();
        let disc = (4 * cx, 4 * cy, 4 * RADIUS as i32);
        paint_disc_over(row, disc, fill, |_, coverage, under| {
            Some(blend(fill, under, coverage))
        });
        let (ix, iy) = self.icon_xy();
        if (y as i32 - iy).abs() > ICON_REACH {
            return;
        }
        let left = (ix - ICON_REACH).max(0) as usize;
        let right = (ix + ICON_REACH + 1) as usize;
        row.each_over(left..right, |x, under| {
            let coverage = icon(self.icon, 4 * (x as i32 - ix) + 2, 4 * (y as i32 - iy) + 2);
            (coverage > 0).then(|| blend(ink, under, coverage))
        });
    }

    /// The per-pixel original of `paint`, over `under`.
    pub fn pixel(&self, x: usize, y: usize, under: u16) -> u16 {
        let (fill, ink) = self.colors();
        let (cx, cy) = self.corner_xy();
        let coverage = disc(x, y, 4 * cx, 4 * cy, 4 * RADIUS as i32);
        if coverage == 0 {
            return under;
        }
        let mut color = blend(fill, under, coverage);
        let (ix, iy) = self.icon_xy();
        let (dx, dy) = (x as i32 - ix, y as i32 - iy);
        if dx.abs() <= ICON_REACH && dy.abs() <= ICON_REACH {
            let icon = icon(self.icon, 4 * dx + 2, 4 * dy + 2);
            if icon > 0 {
                color = blend(ink, color, icon);
            }
        }
        color
    }
}

/// A green disc with a black icon inside, such as the check over RIDE SAVED.
/// The same drawing as a cue, placed anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Badge {
    pub cx: usize,
    pub cy: usize,
    pub radius: usize,
    pub icon: Icon,
}

impl Badge {
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        if y + self.radius + 1 < self.cy || y > self.cy + self.radius + 1 {
            return;
        }
        let disc = (
            4 * self.cx as i32,
            4 * self.cy as i32,
            4 * self.radius as i32,
        );
        paint_disc_over(row, disc, GREEN, |_, coverage, under| {
            Some(blend(GREEN, under, coverage))
        });
        let dy = y as i32 - self.cy as i32;
        if dy.abs() > ICON_REACH {
            return;
        }
        let (cx, cy) = (self.cx as i32, self.cy as i32);
        let range = (cx - ICON_REACH).max(0) as usize..(cx + ICON_REACH + 1) as usize;
        row.each_over(range, |x, under| {
            let coverage = icon(self.icon, 4 * (x as i32 - cx) + 2, 4 * (y as i32 - cy) + 2);
            (coverage > 0).then(|| blend(BLACK, under, coverage))
        });
    }

    pub fn pixel(&self, x: usize, y: usize, under: u16) -> u16 {
        let (cx, cy) = (self.cx as i32, self.cy as i32);
        let coverage = disc(x, y, 4 * cx, 4 * cy, 4 * self.radius as i32);
        if coverage == 0 {
            return under;
        }
        let color = blend(GREEN, under, coverage);
        let (dx, dy) = (x as i32 - cx, y as i32 - cy);
        if dx.abs() > ICON_REACH || dy.abs() > ICON_REACH {
            return color;
        }
        match icon(self.icon, 4 * dx + 2, 4 * dy + 2) {
            0 => color,
            ink => blend(BLACK, color, ink),
        }
    }
}

/// Coverage 0..=15 of an icon at offset (x, y) from its centre, in quarter
/// pixels.
fn icon(icon: Icon, x: i32, y: i32) -> u8 {
    match icon {
        // A triangle pointing right, its weight shifted left to look centred.
        Icon::Play => triangle(x, y, [(-20, -28), (-20, 28), (28, 0)]),
        Icon::Stop => {
            let inside = 22 - x.abs().max(y.abs());
            edge(inside)
        }
        Icon::Check => stroke(x, y, &[(-24, 0), (-8, 16), (24, -18)], 7),
        Icon::Back => stroke(x, y, &[(10, -24), (-14, 0), (10, 24)], 7),
    }
}

/// Coverage from a signed distance inside an edge, in quarter pixels: one
/// pixel of anti-aliasing centred on the edge.
const fn edge(inside: i32) -> u8 {
    let c = (inside + 2) * 15 / 4;
    if c <= 0 {
        0
    } else if c >= 15 {
        15
    } else {
        c as u8
    }
}

/// A filled triangle, its corners clockwise on screen.
fn triangle(x: i32, y: i32, corners: [(i32, i32); 3]) -> u8 {
    let mut coverage = 15;
    for i in 0..3 {
        let (ax, ay) = corners[i];
        let (bx, by) = corners[(i + 1) % 3];
        let (ex, ey) = (bx - ax, by - ay);
        let length = ((ex * ex + ey * ey) as u32).isqrt() as i32;
        // Negative inside for these corners' winding on screen.
        let inside = (ex * (y - ay) - ey * (x - ax)) / length.max(1);
        coverage = coverage.min(edge(-inside));
    }
    coverage
}

/// A polyline `half` quarter pixels either side of its points.
fn stroke(x: i32, y: i32, points: &[(i32, i32)], half: i32) -> u8 {
    let mut nearest = i32::MAX;
    for pair in points.windows(2) {
        let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
        let (ex, ey) = (bx - ax, by - ay);
        let (px, py) = (x - ax, y - ay);
        let length2 = ex * ex + ey * ey;
        let t = (px * ex + py * ey).clamp(0, length2);
        let (qx, qy) = (ax + ex * t / length2, ay + ey * t / length2);
        let (dx, dy) = (x - qx, y - qy);
        nearest = nearest.min(((dx * dx + dy * dy) as u32).isqrt() as i32);
    }
    edge(half - nearest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::WIDTH;

    #[test]
    fn cues_sit_in_their_corner_and_paint_their_reference() {
        for corner in [Corner::TopLeft, Corner::BottomRight] {
            for icon in [Icon::Play, Icon::Stop, Icon::Check, Icon::Back] {
                let cue = Cue::new(corner, icon);
                let (fill, ink) = cue.colors();
                let mut inked = 0;
                for y in 0..HEIGHT {
                    let mut pixels = [BLACK; WIDTH];
                    cue.paint(&mut Row::new(y, &mut pixels));
                    for (x, &pixel) in pixels.iter().enumerate() {
                        assert_eq!(pixel, cue.pixel(x, y, BLACK), "{cue:?} at {x},{y}");
                        inked += usize::from(pixel == ink);
                    }
                }
                assert!(inked > 20, "{cue:?} draws its icon: {inked}");
                let (x, y) = match corner {
                    Corner::TopLeft => (2, 2),
                    Corner::BottomRight => (WIDTH - 3, HEIGHT - 3),
                };
                assert_eq!(cue.pixel(x, y, BLACK), fill, "{cue:?} fills its corner");
                let (far_x, far_y) = match corner {
                    Corner::TopLeft => (RADIUS + 2, 2),
                    Corner::BottomRight => (WIDTH - RADIUS - 3, HEIGHT - 3),
                };
                assert_eq!(cue.pixel(far_x, far_y, BLACK), BLACK, "{cue:?} stays small");
            }
        }
    }
}
