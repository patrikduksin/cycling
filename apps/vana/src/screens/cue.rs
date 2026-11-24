use super::Device;
use super::chrome::{
    BLACK, GREEN, Look, RAISED, RED, Rect, Row, SECONDARY, Shown, WHITE, blend, disc,
    paint_disc_over, rounded,
};
use super::scaled::Scaled;
use device_api::input::{ALONG, Fixtures, Role, Side};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub edge: Side,
    pub at: u16,
}

const STRIP: (usize, usize) = (44, 7);
const SIGN: (usize, usize) = (32, 20);
const RADIUS: usize = 9;
const TARGET_DEPTH: usize = 40;
const TARGET_GROW: usize = 8;

const fn min(a: usize, b: usize) -> usize {
    if a < b { a } else { b }
}

impl Key {
    pub const fn tab(self, at: &Scaled, sign: bool) -> Rect {
        let (length, depth) = if sign { SIGN } else { STRIP };
        let (length, depth) = (at.unit(length), at.unit(depth));
        let (width, height) = (at.panel.width, at.panel.height);
        let side = match self.edge {
            Side::Left | Side::Right => height,
            Side::Top | Side::Bottom => width,
        };
        let centre = side * self.at as usize / ALONG as usize;
        let start = centre.saturating_sub(length / 2);
        let start = if start + length > side {
            side - length
        } else {
            start
        };
        let (end, screen) = (start + length, at.screen());
        match self.edge {
            Side::Left => Rect {
                top: start,
                right: screen.left + depth,
                bottom: end,
                ..screen
            },
            Side::Right => Rect {
                left: screen.right - depth,
                top: start,
                bottom: end,
                ..screen
            },
            Side::Top => Rect {
                left: start,
                right: end,
                bottom: screen.top + depth,
                ..screen
            },
            Side::Bottom => Rect {
                left: start,
                top: screen.bottom - depth,
                right: end,
                ..screen
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Keys {
    act: Key,
    back: Key,
    move_: Key,
}

impl Keys {
    /// Each role's key. Panics, at compile time in a const, when a role
    /// has no button.
    pub const fn of(buttons: &Fixtures) -> Self {
        Self {
            act: key(buttons, Role::Act),
            back: key(buttons, Role::Back),
            move_: key(buttons, Role::Move),
        }
    }

    pub const fn of_role(&self, role: Role) -> Key {
        match role {
            Role::Act => self.act,
            Role::Back => self.back,
            Role::Move => self.move_,
        }
    }

    /// Panics, at compile time in a const, when two roles' tabs would
    /// overlap on `at`.
    pub const fn assert_apart(&self, at: &Scaled) {
        let tabs = [
            self.act.tab(at, true),
            self.back.tab(at, true),
            self.move_.tab(at, true),
        ];
        let mut a = 0;
        while a < tabs.len() {
            let mut b = a + 1;
            while b < tabs.len() {
                assert!(!overlap(tabs[a], tabs[b]), "two keys' tabs overlap");
                b += 1;
            }
            a += 1;
        }
    }
}

const fn key(buttons: &Fixtures, role: Role) -> Key {
    let list = buttons.as_slice();
    let mut index = 0;
    while index < list.len() {
        let fixture = list[index];
        if let Some(found) = fixture.role
            && found as u8 == role as u8
        {
            return Key {
                edge: fixture.edge,
                at: fixture.at,
            };
        }
        index += 1;
    }
    panic!("a role has no button")
}

const fn overlap(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
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
    Less,
    More,
}

impl Icon {
    pub const fn sign(self) -> bool {
        matches!(self, Self::Less | Self::More)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cue {
    pub role: Role,
    pub icon: Icon,
}

impl Cue {
    pub const fn new(role: Role, icon: Icon) -> Self {
        Self { role, icon }
    }

    pub const fn colors(&self) -> (u16, u16) {
        match self.icon {
            Icon::Play | Icon::Check | Icon::Send | Icon::Calibrate => (GREEN, BLACK),
            Icon::Stop | Icon::Trash => (RED, WHITE),
            Icon::Back => (SECONDARY, BLACK),
            Icon::Less | Icon::More => (RAISED, WHITE),
        }
    }

    pub const fn tab(&self, keys: &Keys, at: &Scaled) -> Rect {
        keys.of_role(self.role).tab(at, self.icon.sign())
    }

    const fn rect(&self, device: &Device) -> Rect {
        self.tab(&device.keys, &device.scaled)
    }

    pub const fn covers(&self, rect: Rect, device: &Device) -> bool {
        overlap(self.rect(device), rect)
    }

    pub const fn target(&self, device: &Device) -> Rect {
        let (tab, at) = (self.rect(device), &device.scaled);
        let (reach, grow) = (at.unit(TARGET_DEPTH), at.unit(TARGET_GROW));
        let screen = at.screen();
        let along = Rect {
            left: tab.left.saturating_sub(grow),
            top: tab.top.saturating_sub(grow),
            right: min(tab.right + grow, screen.right),
            bottom: min(tab.bottom + grow, screen.bottom),
        };
        match device.keys.of_role(self.role).edge {
            Side::Left => Rect {
                left: screen.left,
                right: screen.left + reach,
                ..along
            },
            Side::Right => Rect {
                left: screen.right - reach,
                right: screen.right,
                ..along
            },
            Side::Top => Rect {
                top: screen.top,
                bottom: screen.top + reach,
                ..along
            },
            Side::Bottom => Rect {
                top: screen.bottom - reach,
                bottom: screen.bottom,
                ..along
            },
        }
    }

    pub const fn rows(&self, device: &Device) -> core::ops::Range<usize> {
        let rect = self.rect(device);
        rect.top..rect.bottom
    }

    pub const fn clear(&self, keys: &Keys, at: &Scaled, rect: Rect) -> Rect {
        let tab = self.tab(keys, at);
        if !overlap(tab, rect) {
            return rect;
        }
        match keys.of_role(self.role).edge {
            Side::Left => Rect {
                left: tab.right,
                ..rect
            },
            Side::Right => Rect {
                right: tab.left,
                ..rect
            },
            Side::Top => Rect {
                top: tab.bottom,
                ..rect
            },
            Side::Bottom => Rect {
                bottom: tab.top,
                ..rect
            },
        }
    }

    fn over(
        &self,
        device: &Device,
        (fill, ink): (Shown, Shown),
        x: usize,
        y: usize,
        under: Shown,
    ) -> Option<Shown> {
        let tab = self.rect(device);
        let radius = tab.width().min(tab.height()).min(RADIUS);
        // The box runs past the edge by the radius so only its inner corners
        // round; the left and top edges shift box and point to stay unsigned.
        let edge = device.keys.of_role(self.role).edge;
        let across = matches!(edge, Side::Left | Side::Right);
        let (shift_x, shift_y) = match edge {
            Side::Left => (radius, 0),
            Side::Top => (0, radius),
            Side::Right | Side::Bottom => (0, 0),
        };
        let past = Rect {
            left: tab.left,
            top: tab.top,
            right: tab.right + if across { radius } else { 0 },
            bottom: tab.bottom + if across { 0 } else { radius },
        };
        let coverage = rounded(past, radius, x + shift_x, y + shift_y);
        if coverage == 0 || !tab.contains(x, y) {
            return None;
        }
        let color = blend(fill, under, coverage);
        if !self.icon.sign() {
            return Some(color);
        }
        let (cx, cy) = (tab.center_x() as i32, tab.top.midpoint(tab.bottom) as i32);
        let (dx, dy) = (x as i32 - cx, y as i32 - cy);
        if dx.abs() > ICON_REACH || dy.abs() > ICON_REACH {
            return Some(color);
        }
        match icon(self.icon, 4 * dx + 2, 4 * dy + 2) {
            0 => Some(color),
            sign => Some(blend(ink, color, sign)),
        }
    }

    pub fn paint(&self, row: &mut Row, device: &Device) {
        let y = row.y();
        let tab = self.rect(device);
        if !(tab.top..tab.bottom).contains(&y) {
            return;
        }
        let (fill, ink) = self.colors();
        let colors = (row.ink(fill), row.ink(ink));
        row.each_over(tab.left..tab.right, |x, under| {
            self.over(device, colors, x, y, under)
        });
    }

    /// The per-pixel original of `paint`, over `under`, as `look` shows it.
    pub fn pixel(&self, look: Look, x: usize, y: usize, under: Shown, device: &Device) -> Shown {
        let (fill, ink) = self.colors();
        self.over(device, (look.ink(fill), look.ink(ink)), x, y, under)
            .unwrap_or(under)
    }
}

const ICON_REACH: i32 = 9;

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
        Icon::Less => stroke(x, y, &[(-16, 0), (16, 0)], 6),
        Icon::More => {
            stroke(x, y, &[(-16, 0), (16, 0)], 6).max(stroke(x, y, &[(0, -16), (0, 16)], 6))
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
    fn the_c606_keys_are_back_top_left_and_move_and_act_on_the_bottom_edge() {
        let device = &crate::screens::C606;
        let tab = |role| device.keys.of_role(role).tab(&device.scaled, false);
        let (back, move_, act) = (tab(Role::Back), tab(Role::Move), tab(Role::Act));
        assert_eq!((back.left, back.right), (0, 7), "BACK on the left edge");
        assert!(back.bottom <= device.scaled.panel.height / 2, "{back:?}");
        assert!(back.top >= crate::compose::tiles::status::rows(&device.scaled));
        for tab in [move_, act] {
            assert_eq!(
                (tab.top, tab.bottom),
                (313, 320),
                "{tab:?} on the bottom edge"
            );
        }
        let half = device.scaled.panel.width / 2;
        assert!(move_.right <= half && half <= act.left, "{move_:?} {act:?}");
    }

    fn button(id: u8, role: Role, edge: Side, at: u16) -> Fixture {
        Fixture {
            id,
            role: Some(role),
            edge,
            at,
        }
    }

    #[test]
    fn a_tab_stays_on_its_panel_at_either_end_of_its_edge() {
        let at = crate::screens::scaled::BASE;
        for (edge, at_end) in [(Side::Top, 0), (Side::Right, ALONG), (Side::Bottom, ALONG)] {
            let tab = Key { edge, at: at_end }.tab(&at, true);
            assert!(tab.right <= at.panel.width && tab.bottom <= at.panel.height);
            assert_eq!(
                tab.width().max(tab.height()),
                SIGN.0,
                "{tab:?} keeps its length"
            );
        }
    }

    #[test]
    #[should_panic(expected = "tabs overlap")]
    fn two_keys_whose_tabs_meet_are_refused() {
        let keys = Keys::of(&Fixtures::new(&[
            button(0, Role::Back, Side::Left, 250),
            button(1, Role::Move, Side::Bottom, 450),
            button(2, Role::Act, Side::Bottom, 550),
        ]));
        keys.assert_apart(&crate::screens::scaled::BASE);
    }

    #[test]
    fn a_tile_beside_a_tab_ends_where_the_tab_begins() {
        let device = &crate::screens::C606;
        let (keys, at) = (&device.keys, &device.scaled);
        let bottom = Rect {
            left: 0,
            top: 260,
            right: 240,
            bottom: 320,
        };
        let go = Cue::new(Role::Act, Icon::Play);
        assert_eq!(go.clear(keys, at, bottom).bottom, 313);
        let side = Rect {
            left: 0,
            top: 40,
            right: 120,
            bottom: 140,
        };
        assert_eq!(
            Cue::new(Role::Back, Icon::Back).clear(keys, at, side).left,
            7
        );
        let away = Rect {
            top: 0,
            bottom: 40,
            ..side
        };
        assert_eq!(Cue::new(Role::Back, Icon::Back).clear(keys, at, away), away);
    }

    #[test]
    fn tabs_sit_on_their_edge_and_paint_their_reference() {
        const UNDER: u16 = 0x1082;
        let panels = [(240, 320), (240, 400), (180, 240)];
        let elsewhere = Fixtures::new(&[
            button(4, Role::Back, Side::Top, 700),
            button(5, Role::Act, Side::Left, 800),
            button(6, Role::Move, Side::Right, 100),
        ]);
        for ((width, height), buttons) in panels.into_iter().flat_map(|panel| {
            [crate::screens::limits::C606_BUTTONS, elsewhere].map(|buttons| (panel, buttons))
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
                Icon::Less,
                Icon::More,
            ];
            for (role, icon) in Role::ALL
                .into_iter()
                .flat_map(|role| icons.map(|icon| (role, icon)))
            {
                let cue = Cue::new(role, icon);
                let tab = cue.rect(device);
                let (fill, ink) = cue.colors();
                let (mut filled, mut inked) = (0, 0);
                for y in 0..height {
                    let mut pixels = std::vec![UNDER; width];
                    cue.paint(&mut Row::new(y, &mut pixels), device);
                    for (x, &pixel) in pixels.iter().enumerate() {
                        assert_eq!(
                            pixel,
                            cue.pixel(Look::PLAIN, x, y, Shown(UNDER), device).0,
                            "{cue:?} at {x},{y}"
                        );
                        assert!(pixel == UNDER || tab.contains(x, y), "{cue:?} at {x},{y}");
                        filled += usize::from(pixel == fill);
                        inked += usize::from(pixel == ink && pixel != fill);
                    }
                }
                assert!(
                    filled * 2 > tab.width() * tab.height(),
                    "{cue:?} fills its tab"
                );
                assert_eq!(inked > 10, icon.sign(), "{cue:?} carries a sign: {inked}");
            }
        }
    }
}
