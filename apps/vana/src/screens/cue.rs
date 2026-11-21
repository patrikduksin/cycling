//! Button cues: a quarter disc in the corner beside a physical button, with
//! an icon of what that button does on this screen.
//!
//! Cues appear only on screens where the rider decides something about the
//! ride (ready, paused, end ride, ride saved), never on live ride pages and
//! never as words. Their colour follows the palette: `GREEN` disc for the
//! action that moves forward (start, resume, save, done), a `RAISED` disc for
//! stepping back or ending, a `RED` one for throwing a ride away. See
//! `docs/ui.md`, "Button cues".
use super::Device;
use super::chrome::{
    BLACK, GREEN, Look, RAISED, RED, Rect, Row, Shown, WHITE, blend, disc, paint_disc_over,
};
use super::scaled::{Panel, Track};
use device_api::input::{ALONG, Fixtures, Role, Side};

const fn in_middle_third(at: u16) -> bool {
    const FIRST: u16 = ALONG / 3 + 1;
    const LAST: u16 = ALONG - ALONG / 3 - 1;
    matches!(at, FIRST..=LAST)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// The corner nearest a button `at` thousandths along `edge`. A button
    /// in the middle third of its side has no corner: a cue beside the
    /// middle of a side is not designed yet, and `cycling devices` refuses it.
    pub const fn nearest(edge: Side, at: u16) -> Self {
        assert!(
            !in_middle_third(at),
            "a cue beside the middle of a side is not designed yet"
        );
        let far = at > ALONG / 2;
        match (edge, far) {
            (Side::Left | Side::Top, false) => Self::TopLeft,
            (Side::Left, true) | (Side::Bottom, false) => Self::BottomLeft,
            (Side::Right, false) | (Side::Top, true) => Self::TopRight,
            (Side::Right | Side::Bottom, true) => Self::BottomRight,
        }
    }

    pub const fn top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopRight)
    }

    pub const fn left(self) -> bool {
        matches!(self, Self::TopLeft | Self::BottomLeft)
    }

    const fn clear_columns(self, rect: Rect, panel: Panel) -> Rect {
        let reach = disc_reach_in_rounded_up(if self.top() {
            rect.top
        } else {
            panel.height - rect.bottom
        });
        if self.left() {
            Rect {
                left: if rect.left < reach { reach } else { rect.left },
                ..rect
            }
        } else {
            let edge = panel.width - reach;
            Rect {
                right: if edge < rect.right { edge } else { rect.right },
                ..rect
            }
        }
    }

    const fn clear_rows(self, rect: Rect, panel: Panel, gap: usize) -> Rect {
        let reach = gap
            + disc_reach_in_rounded_up(if self.left() {
                rect.left
            } else {
                panel.width - rect.right
            });
        if self.top() {
            Rect {
                top: if rect.top < reach { reach } else { rect.top },
                ..rect
            }
        } else {
            let edge = panel.height - reach;
            Rect {
                bottom: if edge < rect.bottom {
                    edge
                } else {
                    rect.bottom
                },
                ..rect
            }
        }
    }

    const fn inset(self, panel: Panel, inset: i32) -> (i32, i32) {
        let (width, height) = (panel.width as i32, panel.height as i32);
        (
            if self.left() { inset } else { width - inset },
            if self.top() { inset } else { height - inset },
        )
    }
}

/// The corner beside each role's button on one device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Corners {
    act: Corner,
    back: Corner,
    move_: Corner,
}

impl Corners {
    /// Each role's corner, nearest its button. Refused at compile time when
    /// a role has no button, or when two roles' cues would share a corner.
    pub const fn of(buttons: &Fixtures) -> Self {
        let (act, back, move_) = (
            nearest(buttons, Role::Act),
            nearest(buttons, Role::Back),
            nearest(buttons, Role::Move),
        );
        assert!(
            act as u8 != back as u8 && act as u8 != move_ as u8 && back as u8 != move_ as u8,
            "two roles' buttons sit nearest the same corner"
        );
        Self { act, back, move_ }
    }

    pub const fn of_role(&self, role: Role) -> Corner {
        match role {
            Role::Act => self.act,
            Role::Back => self.back,
            Role::Move => self.move_,
        }
    }

    /// `rect` narrowed so each of `cues` stays out of it on `panel`.
    pub const fn columns_clear_of(&self, cues: &[Cue], rect: Rect, panel: Panel) -> Rect {
        let mut rect = rect;
        let mut index = 0;
        while index < cues.len() {
            rect = self.of_role(cues[index].role).clear_columns(rect, panel);
            index += 1;
        }
        rect
    }

    /// `rect` cut short so each of `cues` stays `gap` rows clear of it on
    /// `panel`.
    pub const fn rows_clear_of(&self, cues: &[Cue], rect: Rect, panel: Panel, gap: usize) -> Rect {
        let mut rect = rect;
        let mut index = 0;
        while index < cues.len() {
            rect = self.of_role(cues[index].role).clear_rows(rect, panel, gap);
            index += 1;
        }
        rect
    }
}

const fn nearest(buttons: &Fixtures, role: Role) -> Corner {
    let list = buttons.as_slice();
    let mut index = 0;
    while index < list.len() {
        let fixture = list[index];
        if let Some(found) = fixture.role
            && found as u8 == role as u8
        {
            return Corner::nearest(fixture.edge, fixture.at);
        }
        index += 1;
    }
    panic!("a role has no button")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Start or resume the ride.
    Play,
    /// End the ride.
    Stop,
    /// Confirm or finish.
    Check,
    /// Go back without changing anything.
    Back,
    /// Send to the phone.
    Send,
    /// Discard or delete a ride, or forget a sensor.
    Trash,
    /// Zero a power meter: a target.
    Calibrate,
}

/// Columns across a cue screen that keep its text clear of the cues on both
/// sides.
pub const CLEAR_OF_CUES: [Track; 3] = [Track::Unit(24), Track::Fr(1), Track::Unit(24)];

/// The visible quarter disc's radius.
pub const RADIUS: usize = 42;

const fn disc_reach_in_rounded_up(from_edge: usize) -> usize {
    if from_edge >= RADIUS {
        return 0;
    }
    let span = RADIUS * RADIUS - from_edge * from_edge;
    let root = span.isqrt();
    if root * root < span { root + 1 } else { root }
}
/// The icon's centre, this far in from both edges of its corner.
const ICON_INSET: i32 = 17;
/// The box around the icon centre that the icon can touch, in pixels.
const ICON_REACH: i32 = 9;

/// Whether any of `cues` sits in a top corner, where the status bar would
/// cut it: such a screen shows without the bar.
pub fn any_at_top(cues: &[Cue], device: &Device) -> bool {
    cues.iter().any(|cue| cue.corner(device).top())
}

/// A cue beside the button of `role`, which answers the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cue {
    pub role: Role,
    pub icon: Icon,
}

impl Cue {
    pub const fn new(role: Role, icon: Icon) -> Self {
        Self { role, icon }
    }

    /// Forward actions are green with a black icon, throwing a ride away is
    /// red with a white one; others are raised grey with a white one.
    pub const fn colors(&self) -> (u16, u16) {
        match self.icon {
            Icon::Play | Icon::Check | Icon::Send | Icon::Calibrate => (GREEN, BLACK),
            Icon::Stop | Icon::Back => (RAISED, WHITE),
            Icon::Trash => (RED, WHITE),
        }
    }

    /// The corner the cue sits in on `device`.
    pub const fn corner(&self, device: &Device) -> Corner {
        device.corners.of_role(self.role)
    }

    /// Whether the disc reaches into `rect`.
    pub const fn covers(&self, rect: Rect, device: &Device) -> bool {
        let (cx, cy) = self.corner(device).inset(device.scaled.panel, 0);
        let (cx, cy) = (cx as usize, cy as usize);
        let dx = if cx < rect.left {
            rect.left - cx
        } else {
            cx.saturating_sub(rect.right)
        };
        let dy = if cy < rect.top {
            rect.top - cy
        } else {
            cy.saturating_sub(rect.bottom)
        };
        dx * dx + dy * dy < RADIUS * RADIUS
    }

    pub const fn rows(&self, device: &Device) -> core::ops::Range<usize> {
        let height = device.scaled.panel.height;
        if self.corner(device).top() {
            0..RADIUS + 1
        } else {
            height - RADIUS - 1..height
        }
    }

    pub fn paint(&self, row: &mut Row, device: &Device) {
        let y = row.y();
        if !self.rows(device).contains(&y) {
            return;
        }
        let (corner, panel) = (self.corner(device), device.scaled.panel);
        let (fill, ink) = self.colors();
        let (cx, cy) = corner.inset(panel, 0);
        let disc = (4 * cx, 4 * cy, 4 * RADIUS as i32);
        paint_disc_over(row, disc, fill);
        let (ix, iy) = corner.inset(panel, ICON_INSET);
        if (y as i32 - iy).abs() > ICON_REACH {
            return;
        }
        let left = (ix - ICON_REACH).max(0) as usize;
        let right = (ix + ICON_REACH + 1) as usize;
        let ink = row.ink(ink);
        row.each_over(left..right, |x, under| {
            let coverage = icon(self.icon, 4 * (x as i32 - ix) + 2, 4 * (y as i32 - iy) + 2);
            (coverage > 0).then(|| blend(ink, under, coverage))
        });
    }

    /// The per-pixel original of `paint`, over `under`, as `look` shows it.
    pub fn pixel(&self, look: Look, x: usize, y: usize, under: Shown, device: &Device) -> Shown {
        let (corner, panel) = (self.corner(device), device.scaled.panel);
        let (fill, ink) = self.colors();
        let (fill, ink) = (look.ink(fill), look.ink(ink));
        let (cx, cy) = corner.inset(panel, 0);
        let coverage = disc(x, y, 4 * cx, 4 * cy, 4 * RADIUS as i32);
        if coverage == 0 {
            return under;
        }
        let mut color = blend(fill, under, coverage);
        let (ix, iy) = corner.inset(panel, ICON_INSET);
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
        paint_disc_over(row, disc, GREEN);
        let dy = y as i32 - self.cy as i32;
        if dy.abs() > ICON_REACH {
            return;
        }
        let (cx, cy) = (self.cx as i32, self.cy as i32);
        let range = (cx - ICON_REACH).max(0) as usize..(cx + ICON_REACH + 1) as usize;
        let black = row.ink(BLACK);
        row.each_over(range, |x, under| {
            let coverage = icon(self.icon, 4 * (x as i32 - cx) + 2, 4 * (y as i32 - cy) + 2);
            (coverage > 0).then(|| blend(black, under, coverage))
        });
    }

    pub fn pixel(&self, look: Look, x: usize, y: usize, under: Shown) -> Shown {
        let (cx, cy) = (self.cx as i32, self.cy as i32);
        let coverage = disc(x, y, 4 * cx, 4 * cy, 4 * self.radius as i32);
        if coverage == 0 {
            return under;
        }
        let color = blend(look.ink(GREEN), under, coverage);
        let (dx, dy) = (x as i32 - cx, y as i32 - cy);
        if dx.abs() > ICON_REACH || dy.abs() > ICON_REACH {
            return color;
        }
        match icon(self.icon, 4 * dx + 2, 4 * dy + 2) {
            0 => color,
            ink => blend(look.ink(BLACK), color, ink),
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
        // An arrow up and away: the shaft, then its head.
        Icon::Send => stroke(x, y, &[(0, 26), (0, -20)], 7).max(stroke(
            x,
            y,
            &[(-18, -2), (0, -22), (18, -2)],
            7,
        )),
        // A bin: the lid and its handle, then the body narrowing down.
        Icon::Trash => stroke(x, y, &[(-24, -16), (24, -16)], 5)
            .max(stroke(x, y, &[(-8, -16), (-8, -24), (8, -24), (8, -16)], 5))
            .max(stroke(x, y, &[(-16, -8), (-12, 26), (12, 26), (16, -8)], 5)),
        Icon::Calibrate => {
            let distance = ((x * x + y * y) as u32).isqrt() as i32;
            edge((9 - 2 * (distance - 19).abs()) / 2).max(edge(7 - distance))
        }
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

// `stroke` takes `edge` as zero from two quarter pixels outside on.
const _: () = assert!(edge(-2) == 0 && edge(-1) > 0);

/// A polyline `half` quarter pixels either side of its points.
fn stroke(x: i32, y: i32, points: &[(i32, i32)], half: i32) -> u8 {
    // `edge` is zero from `reach` outside the line on, so a segment whose
    // box is that far away needs no divisions, and the rest one square root.
    let reach = half + 2;
    let reach2 = (reach * reach) as u32;
    let mut nearest2 = reach2;
    for pair in points.windows(2) {
        let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
        if x < ax.min(bx) - reach
            || x > ax.max(bx) + reach
            || y < ay.min(by) - reach
            || y > ay.max(by) + reach
        {
            continue;
        }
        let (ex, ey) = (bx - ax, by - ay);
        let (px, py) = (x - ax, y - ay);
        let length2 = ex * ex + ey * ey;
        // A repeated point is a segment of no length: its nearest point is itself.
        let (qx, qy) = if length2 == 0 {
            (ax, ay)
        } else {
            let t = (px * ex + py * ey).clamp(0, length2);
            (ax + ex * t / length2, ay + ey * t / length2)
        };
        let (dx, dy) = (x - qx, y - qy);
        nearest2 = nearest2.min((dx * dx + dy * dy) as u32);
    }
    if nearest2 >= reach2 {
        return 0;
    }
    edge(half - nearest2.isqrt() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_api::input::Fixture;

    #[test]
    fn strokes_through_a_repeated_point_draw_as_without_it() {
        let line = [(-24, 0), (-8, 16), (24, -18)];
        let repeated = [(-24, 0), (-24, 0), (-8, 16), (-8, 16), (24, -18)];
        for y in -40..=40 {
            for x in -40..=40 {
                assert_eq!(
                    stroke(x, y, &repeated, 7),
                    stroke(x, y, &line, 7),
                    "{x},{y}"
                );
            }
        }
        let dot = [(4, -4), (4, -4)];
        assert_eq!(stroke(4, -4, &dot, 7), 15);
        assert!(stroke(4 + 8, -4, &dot, 7) > 0);
        assert_eq!(stroke(4 + 9, -4, &dot, 7), 0);
    }

    #[test]
    fn the_calibrate_target_is_a_ring_around_a_dot() {
        let target = |x, y| icon(Icon::Calibrate, x, y);
        assert_eq!(target(0, 0), 15, "the dot");
        assert_eq!(target(12, 0), 0, "the gap");
        assert_eq!(target(19, 0), 15, "the ring");
        assert_eq!(target(0, -19), 15);
        assert_eq!(target(-14, 13), 15);
        assert_eq!(target(26, 0), 0, "nothing outside");
    }

    #[test]
    fn the_base_buttons_put_back_top_left_move_bottom_left_and_act_bottom_right() {
        let corners = crate::screens::C606.corners;
        assert_eq!(corners.of_role(Role::Back), Corner::TopLeft);
        assert_eq!(corners.of_role(Role::Move), Corner::BottomLeft);
        assert_eq!(corners.of_role(Role::Act), Corner::BottomRight);
    }

    #[test]
    fn a_button_takes_the_corner_nearest_it() {
        let cases = [
            (Side::Left, 0, Corner::TopLeft),
            (Side::Left, 333, Corner::TopLeft),
            (Side::Left, 667, Corner::BottomLeft),
            (Side::Right, 100, Corner::TopRight),
            (Side::Right, ALONG, Corner::BottomRight),
            (Side::Top, 200, Corner::TopLeft),
            (Side::Top, 800, Corner::TopRight),
            (Side::Bottom, 300, Corner::BottomLeft),
            (Side::Bottom, 900, Corner::BottomRight),
        ];
        for (edge, at, corner) in cases {
            assert_eq!(Corner::nearest(edge, at), corner, "{edge:?} {at}");
        }
    }

    fn button(id: u8, role: Role, edge: Side, at: u16) -> Fixture {
        Fixture {
            id,
            role: Some(role),
            edge,
            at,
        }
    }

    const OFF_C606_CORNERS: Fixtures = Fixtures::new(&[
        Fixture {
            id: 4,
            role: None,
            edge: Side::Left,
            at: 100,
        },
        Fixture {
            id: 5,
            role: Some(Role::Act),
            edge: Side::Left,
            at: 800,
        },
        Fixture {
            id: 6,
            role: Some(Role::Move),
            edge: Side::Bottom,
            at: 900,
        },
        Fixture {
            id: 7,
            role: Some(Role::Back),
            edge: Side::Top,
            at: 700,
        },
    ]);

    #[test]
    fn a_cue_follows_its_role_s_button() {
        let corners = Corners::of(&OFF_C606_CORNERS);
        assert_eq!(corners.of_role(Role::Act), Corner::BottomLeft);
        assert_eq!(corners.of_role(Role::Move), Corner::BottomRight);
        assert_eq!(corners.of_role(Role::Back), Corner::TopRight);
        let act_on_the_left = Fixtures::new(&[
            button(0, Role::Back, Side::Left, 250),
            button(1, Role::Act, Side::Left, 750),
            button(2, Role::Move, Side::Right, 750),
        ]);
        let corners = Corners::of(&act_on_the_left);
        assert_eq!(corners.of_role(Role::Act), Corner::BottomLeft);
        assert_eq!(corners.of_role(Role::Move), Corner::BottomRight);
    }

    #[test]
    #[should_panic(expected = "not designed yet")]
    fn a_button_mid_side_has_no_corner() {
        Corner::nearest(Side::Right, ALONG / 2);
    }

    #[test]
    #[should_panic(expected = "the same corner")]
    fn two_roles_may_not_share_a_corner() {
        Corners::of(&Fixtures::new(&[
            button(0, Role::Back, Side::Left, 250),
            button(1, Role::Move, Side::Left, 300),
            button(2, Role::Act, Side::Right, 750),
        ]));
    }

    #[test]
    fn cues_sit_in_their_corner_and_paint_their_reference() {
        let panels = [(240, 320), (240, 400), (180, 240)];
        let corners = [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ];
        for ((width, height), buttons) in panels.into_iter().flat_map(|panel| {
            [crate::screens::C606_BUTTONS, OFF_C606_CORNERS].map(|buttons| (panel, buttons))
        }) {
            let device = std::boxed::Box::leak(std::boxed::Box::new(Device::of(
                crate::screens::scaled::Scaled::of(width, height),
                &buttons,
            )));
            let icons = [
                Icon::Play,
                Icon::Stop,
                Icon::Check,
                Icon::Back,
                Icon::Send,
                Icon::Trash,
                Icon::Calibrate,
            ];
            for (role, icon) in Role::ALL
                .into_iter()
                .flat_map(|role| icons.map(|icon| (role, icon)))
            {
                let cue = Cue::new(role, icon);
                let corner = cue.corner(device);
                assert!(corners.contains(&corner));
                let (fill, ink) = cue.colors();
                let mut inked = 0;
                for y in 0..height {
                    let mut pixels = std::vec![BLACK; width];
                    cue.paint(&mut Row::new(y, &mut pixels), device);
                    for (x, &pixel) in pixels.iter().enumerate() {
                        assert_eq!(
                            pixel,
                            cue.pixel(Look::PLAIN, x, y, Shown(BLACK), device).0,
                            "{cue:?} at {x},{y}"
                        );
                        inked += usize::from(pixel == ink);
                    }
                }
                assert!(inked > 20, "{cue:?} draws its icon: {inked}");
                let (near, far) = ((2, 3), (RADIUS + 2, RADIUS + 3));
                let x = |(left, right)| if corner.left() { left } else { width - right };
                let y = |(top, bottom)| if corner.top() { top } else { height - bottom };
                assert_eq!(
                    cue.pixel(Look::PLAIN, x(near), y(near), Shown(BLACK), device)
                        .0,
                    fill,
                    "{cue:?} fills its {corner:?} corner"
                );
                assert_eq!(
                    cue.pixel(Look::PLAIN, x(far), y(near), Shown(BLACK), device)
                        .0,
                    BLACK,
                    "{cue:?} stays small"
                );
            }
        }
    }
}
