//! Layout from the panel's geometry. A panel lays out at one scale step,
//! from 3/4 to 3/2 in quarters; [`Track`]s split a box into rows or columns
//! at that step, and [`Scaled::faces`] holds the faces generated for it. At
//! scale 1, the C606's, every edge is the pixel offset a 240 × 320 screen
//! used before.
use super::chrome::Rect;
use super::icons::{ART_0_75, ART_1, Art};
use firmware_shell::rendering::font::{Face, FaceRole, Faces, SCALE_0_75, SCALE_1};

/// The panel every layout's numbers were drawn for.
pub const BASE_PANEL: Panel = Panel {
    width: 240,
    height: 320,
};

/// Every panel this build can meet is the base panel. Then no frame is ever
/// scaled, and a picture's size is known when it compiles, so the shell
/// leaves its scaling path out.
pub const ONLY_BASE: bool = {
    let panels = super::limits::PANELS;
    let mut i = 0;
    while i < panels.len() {
        if panels[i].0 != BASE_PANEL.width || panels[i].1 != BASE_PANEL.height {
            break;
        }
        i += 1;
    }
    i == panels.len()
};

/// The largest `$size` over the `$panels` VANA can lay out, `$at` naming
/// each one's `Scaled`: the bound of an array that holds a region of every
/// panel the build meets. It bounds each panel's own layout, never a scale-1
/// offset off the largest panel, which a smaller step can outgrow.
macro_rules! largest {
    ($panels:expr, |$at:ident| $size:expr) => {{
        let panels: &[(usize, usize)] = $panels;
        let mut largest = 0;
        let mut i = 0;
        while i < panels.len() {
            if let Some($at) = $crate::screens::scaled::Scaled::fitting(panels[i].0, panels[i].1) {
                let size = $size;
                if size > largest {
                    largest = size;
                }
            }
            i += 1;
        }
        largest
    }};
}
pub(crate) use largest;

/// A scale step in quarters, from 3/4 to 3/2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scale(u8);

impl Scale {
    pub const ONE: Self = Self(4);

    /// The step a panel lays out at: the smaller of its ratios to 240 × 320,
    /// floored to a quarter and capped at 3/2. None below 3/4.
    pub const fn of(panel: Panel) -> Option<Self> {
        let across = 4 * panel.width / BASE_PANEL.width;
        let down = 4 * panel.height / BASE_PANEL.height;
        let quarters = if across < down { across } else { down };
        match quarters {
            0..=2 => None,
            3..=6 => Some(Self(quarters as u8)),
            _ => Some(Self(6)),
        }
    }

    /// `units` at this step, rounded half up.
    pub const fn px(self, units: usize) -> usize {
        (units * self.0 as usize + 2) / 4
    }

    /// The step as a factor, for a mockup's fractional pixels.
    pub const fn factor(self) -> f32 {
        self.0 as f32 / 4.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Panel {
    pub width: usize,
    pub height: usize,
}

/// A panel and the step it lays out at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scaled {
    pub panel: Panel,
    pub scale: Scale,
    /// The faces generated for this step. Chosen when the layout is built,
    /// so firmware links only the set its panel uses.
    pub faces: &'static Faces,
    /// The icons rasterized for this step, chosen the same way.
    pub art: &'static Art,
}

/// The 240 × 320 canvas that screens not yet laid out from `Scaled` draw,
/// fitted to other panels by the shell's scaler.
pub const BASE: Scaled = Scaled::of(BASE_PANEL.width, BASE_PANEL.height);

impl Scaled {
    /// A panel VANA can lay out; refused at compile time below 3/4 or at a
    /// step no faces are generated for.
    pub const fn of(width: usize, height: usize) -> Self {
        match Self::fitting(width, height) {
            Some(scaled) => scaled,
            None => panic!("the panel lays out below 3/4, or at a step with no faces"),
        }
    }

    /// `of`, or none for a panel VANA cannot lay out.
    pub const fn fitting(width: usize, height: usize) -> Option<Self> {
        let panel = Panel { width, height };
        let Some(scale) = Scale::of(panel) else {
            return None;
        };
        let (faces, art) = match scale.0 {
            3 => (&SCALE_0_75, &ART_0_75),
            4 => (&SCALE_1, &ART_1),
            _ => return None,
        };
        Some(Self {
            panel,
            scale,
            faces,
            art,
        })
    }

    /// `n` units, rounded half up: a lone offset measured from a region's edge.
    pub const fn unit(&self, n: usize) -> usize {
        self.scale.px(n)
    }

    pub const fn face(&self, role: FaceRole) -> &'static Face {
        self.faces.get(role)
    }

    /// The whole panel.
    pub const fn screen(&self) -> Rect {
        Rect {
            left: 0,
            top: 0,
            right: self.panel.width,
            bottom: self.panel.height,
        }
    }
}

/// One track of a split: `Unit` scaled units, a face's `Line` height,
/// `Cap` height, or `Rise`, its capitals and the rows its glyphs rise over
/// them, in that face's pixels, or `Fr`, a share of what is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Unit(u16),
    Line(FaceRole),
    Cap(FaceRole),
    Rise(FaceRole),
    Fr(u8),
}

impl Rect {
    /// `self`'s rows between `columns`' left and right edges.
    pub const fn with_columns(self, columns: Rect) -> Rect {
        Rect {
            left: columns.left,
            right: columns.right,
            ..self
        }
    }

    /// `self` split top to bottom, `gap` between tracks.
    pub const fn rows<const N: usize>(
        self,
        tracks: [Track; N],
        gap: Track,
        at: &Scaled,
    ) -> [Rect; N] {
        let spans = split(self.top, self.bottom, &tracks, gap, at);
        let mut out = [self; N];
        let mut i = 0;
        while i < N {
            out[i].top = spans[i].0;
            out[i].bottom = spans[i].1;
            i += 1;
        }
        out
    }

    /// `self` split left to right, `gap` between tracks.
    pub const fn cols<const N: usize>(
        self,
        tracks: [Track; N],
        gap: Track,
        at: &Scaled,
    ) -> [Rect; N] {
        let spans = split(self.left, self.right, &tracks, gap, at);
        let mut out = [self; N];
        let mut i = 0;
        while i < N {
            out[i].left = spans[i].0;
            out[i].right = spans[i].1;
            i += 1;
        }
        out
    }
}

const fn nth<const N: usize>(tracks: &[Track; N], gap: Track, i: usize) -> Track {
    if i.is_multiple_of(2) {
        tracks[i / 2]
    } else {
        gap
    }
}

const fn split<const N: usize>(
    start: usize,
    end: usize,
    tracks: &[Track; N],
    gap: Track,
    at: &Scaled,
) -> [(usize, usize); N] {
    let count = if N == 0 { 0 } else { 2 * N - 1 };
    let (mut units, mut pixels, mut weight) = (0, 0, 0);
    let mut i = 0;
    while i < count {
        match nth(tracks, gap, i) {
            Track::Unit(n) => units += n as usize,
            Track::Line(role) => pixels += at.face(role).height as usize,
            Track::Cap(role) => pixels += at.face(role).cap as usize,
            Track::Rise(role) => {
                let face = at.face(role);
                pixels += face.top as usize + face.cap as usize;
            }
            Track::Fr(share) => weight += share as usize,
        }
        i += 1;
    }
    let fixed = at.unit(units) + pixels;
    assert!(fixed <= end - start, "the fixed tracks do not fit");
    let rest = end - start - fixed;
    let mut spans = [(start, start); N];
    let (mut units, mut pixels, mut shared, mut given) = (0, 0, 0, 0);
    let mut i = 0;
    while i < count {
        let from = start + at.unit(units) + pixels + given;
        match nth(tracks, gap, i) {
            Track::Unit(n) => units += n as usize,
            Track::Line(role) => pixels += at.face(role).height as usize,
            Track::Cap(role) => pixels += at.face(role).cap as usize,
            Track::Rise(role) => {
                let face = at.face(role);
                pixels += face.top as usize + face.cap as usize;
            }
            Track::Fr(share) => {
                shared += share as usize;
                given = if shared == weight {
                    rest
                } else {
                    given + rest * share as usize / weight
                };
            }
        }
        if i.is_multiple_of(2) {
            spans[i / 2] = (from, start + at.unit(units) + pixels + given);
        }
        i += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::Track::{Cap, Fr, Line, Unit};
    use super::*;

    static SMALL: Scaled = Scaled::of(180, 240);
    static WIDE: Scaled = Scaled::of(240, 400);

    fn tops<const N: usize>(rects: [Rect; N]) -> [(usize, usize); N] {
        rects.map(|r| (r.top, r.bottom))
    }

    #[test]
    fn the_scale_is_the_smaller_ratio_floored_to_a_quarter() {
        let of = |width, height| Scale::of(Panel { width, height }).map(|s| s.0);
        assert_eq!(of(240, 320), Some(4));
        assert_eq!(of(240, 400), Some(4), "taller does not scale up");
        assert_eq!(of(180, 240), Some(3));
        assert_eq!(of(200, 320), Some(3));
        assert_eq!(of(179, 240), None, "below 3/4 is refused");
        assert_eq!(of(800, 800), Some(6), "3/2 at most");
        assert!(Scaled::fitting(32, 24).is_none());
        assert!(Scaled::fitting(300, 400).is_none(), "5/4 has no faces yet");
    }

    #[test]
    fn units_round_half_up() {
        let three = Scale(3);
        assert_eq!(
            [0, 1, 2, 3, 4, 14, 22].map(|n| three.px(n)),
            [0, 1, 2, 2, 3, 11, 17]
        );
        assert_eq!(Scale::ONE.px(37), 37);
    }

    #[test]
    fn at_scale_one_every_edge_is_the_offset_it_names() {
        let screen = BASE.screen();
        let rows = screen.rows([Unit(22), Unit(2), Fr(1)], Unit(0), &BASE);
        assert_eq!(tops(rows), [(0, 22), (22, 24), (24, 320)]);
        let gapped = screen.rows([Unit(4), Unit(34), Unit(70)], Unit(1), &BASE);
        assert_eq!(tops(gapped), [(0, 4), (5, 39), (40, 110)]);
    }

    #[test]
    fn units_never_drift_at_three_quarters() {
        let rows = SMALL.screen().rows([Unit(1); 24], Unit(0), &SMALL);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.bottom, SMALL.unit(i + 1), "edge {i}");
        }
        let full = SMALL
            .screen()
            .rows([Unit(22), Unit(2), Unit(296)], Unit(0), &SMALL);
        assert_eq!(
            full[2].bottom, 240,
            "the C606's stack fills 180 × 240 exactly"
        );
    }

    #[test]
    fn faces_add_their_own_pixels() {
        let value = BASE.face(FaceRole::Value);
        let rows = BASE.screen().rows(
            [Unit(4), Line(FaceRole::Value), Cap(FaceRole::Value)],
            Unit(0),
            &BASE,
        );
        assert_eq!(rows[1].bottom, 4 + value.height as usize);
        assert_eq!(rows[2].bottom, 4 + (value.height + value.cap) as usize);
        let small = SMALL.face(FaceRole::Value);
        let scaled = SMALL
            .screen()
            .rows([Unit(4), Line(FaceRole::Value)], Unit(0), &SMALL);
        assert_eq!(
            scaled[1],
            Rect {
                top: 3,
                bottom: 3 + small.height as usize,
                ..SMALL.screen()
            }
        );
    }

    #[test]
    fn fr_shares_the_rest_and_the_last_takes_the_leftover() {
        let rect = Rect {
            left: 0,
            top: 0,
            right: 10,
            bottom: 10,
        };
        let rows = rect.rows([Fr(1), Fr(1), Fr(1)], Unit(0), &BASE);
        assert_eq!(tops(rows), [(0, 3), (3, 6), (6, 10)]);
        let eleven = Rect { bottom: 11, ..rect };
        let rows = eleven.rows([Fr(1), Fr(1), Fr(1)], Unit(0), &BASE);
        assert_eq!(tops(rows), [(0, 3), (3, 6), (6, 11)]);
        let weighted = rect.rows([Fr(1), Unit(4), Fr(2)], Unit(0), &BASE);
        assert_eq!(tops(weighted), [(0, 2), (2, 6), (6, 10)]);
        let wide = WIDE
            .screen()
            .rows([Unit(22), Fr(1), Unit(40)], Unit(0), &WIDE);
        assert_eq!(tops(wide), [(0, 22), (22, 360), (360, 400)]);
    }

    #[test]
    fn columns_split_the_same_way() {
        let cols = SMALL.screen().cols([Unit(28), Fr(1)], Unit(0), &SMALL);
        assert_eq!(cols.map(|c| (c.left, c.right)), [(0, 21), (21, 180)]);
    }

    #[test]
    #[should_panic(expected = "the fixed tracks do not fit")]
    fn tracks_that_overflow_are_refused() {
        let _ = SMALL.screen().rows([Unit(300), Unit(100)], Unit(0), &SMALL);
    }
}
