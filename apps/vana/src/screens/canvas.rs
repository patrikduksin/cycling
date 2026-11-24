//! A prepared screen: text runs and shapes laid out once per frame, then
//! painted a display row at a time.
//!
//! Screens build a `Canvas` in `prepare` from measured text (`Line::fit`)
//! and the shapes of `docs/ui.md`, so placement is decided once and painting
//! only fills spans and blends glyph runs. `pixel` is the per-pixel original
//! that tests hold `paint` to, and `assert_laid_out` is the layout sweep's
//! check.
use super::chrome::{
    Align, Label, Line, Rect, Ring, Row, SECONDARY, blend, paint_disc_over, paint_rounded_over,
    rounded,
};
#[cfg(test)]
use super::chrome::{Look, Shown, disc};
use super::icons::StaticIcon;
use super::scaled::Scaled;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};
use firmware_shell::shell::Parts;
use heapless::Vec;

/// Text on a canvas: up to 24 characters.
pub type Run = Label<24>;

/// Columns kept clear between neighbouring runs, their spill included.
pub const GAP: usize = 8 + SPILL;

#[derive(Clone, Copy, Debug, Hash)]
pub enum Shape {
    /// A surface text may sit on: a field, a pad, a selected card.
    Panel {
        rect: Rect,
        radius: usize,
        color: u16,
    },
    /// A mark of its own, such as a chart bar: text never covers it.
    Bar {
        rect: Rect,
        radius: usize,
        color: u16,
    },
    /// A bar in two colours: the first left of column `split`, the second
    /// from it, as a zone gauge's segment lit up to where a reading falls.
    Split {
        rect: Rect,
        radius: usize,
        split: usize,
        colors: [u16; 2],
    },
    /// A disc around (cx, cy) of `radius`, all in quarter pixels.
    Disc {
        cx: i32,
        cy: i32,
        radius: i32,
        color: u16,
    },
    /// An arc of a ring with round ends (`chrome::Ring`); `ends` caches
    /// where its ends are, so painting needs no trigonometry.
    Ring {
        ring: Ring,
        ends: [(i32, i32); 2],
        color: u16,
    },
    /// A play triangle filling `rect`, its flat side on the left.
    Play { rect: Rect, color: u16 },
    /// An icon in one colour with its square's top-left corner at
    /// (`left`, `top`): a mark of its own, like a bar.
    Icon {
        icon: StaticIcon,
        left: usize,
        top: usize,
        color: u16,
    },
}

impl Shape {
    /// The box the shape can cover.
    pub fn bounds(&self) -> Rect {
        match *self {
            Self::Panel { rect, .. }
            | Self::Bar { rect, .. }
            | Self::Split { rect, .. }
            | Self::Play { rect, .. } => rect,
            Self::Disc { cx, cy, radius, .. } => Rect {
                left: ((cx - radius - 2) / 4).max(0) as usize,
                top: ((cy - radius - 2) / 4).max(0) as usize,
                right: ((cx + radius + 2) / 4 + 1) as usize,
                bottom: ((cy + radius + 2) / 4 + 1) as usize,
            },
            Self::Ring { ring, .. } => ring.bounds(),
            Self::Icon {
                icon, left, top, ..
            } => icon.ink(left, top),
        }
    }

    /// A ring arc in `color`.
    pub fn ring(ring: Ring, color: u16) -> Self {
        Self::Ring {
            ring,
            ends: ring.ends(),
            color,
        }
    }

    fn paint(&self, row: &mut Row) {
        match *self {
            Self::Panel {
                rect,
                radius,
                color,
            }
            | Self::Bar {
                rect,
                radius,
                color,
            } => paint_bar(row, rect, radius, color),
            Self::Split {
                rect,
                radius,
                split,
                colors: [left, right],
            } => {
                paint_bar(&mut row.within(rect.left..split), rect, radius, left);
                paint_bar(&mut row.within(split..rect.right), rect, radius, right);
            }
            Self::Disc {
                cx,
                cy,
                radius,
                color,
            } => paint_disc_over(row, (cx, cy, radius), color),
            Self::Ring { ring, ends, color } => ring.paint(row, &ends, color),
            Self::Play { rect, color } => {
                if let Some(width) = play_width(rect, row.y()) {
                    row.span(rect.left..rect.left + width, color);
                }
            }
            Self::Icon {
                icon,
                left,
                top,
                color,
            } => icon.paint(row, left, top, color),
        }
    }

    #[cfg(test)]
    fn pixel(&self, look: Look, x: usize, y: usize, under: Shown) -> Option<Shown> {
        let (coverage, color) = match *self {
            Self::Panel {
                rect,
                radius,
                color,
            }
            | Self::Bar {
                rect,
                radius,
                color,
            } => (rounded(rect, fitted(rect, radius), x, y), color),
            Self::Split {
                rect,
                radius,
                split,
                colors: [left, right],
            } => (
                rounded(rect, fitted(rect, radius), x, y),
                if x < split { left } else { right },
            ),
            Self::Disc {
                cx,
                cy,
                radius,
                color,
            } => (disc(x, y, cx, cy, radius), color),
            Self::Ring { ring, ends, color } => (ring.coverage(x, y, &ends), color),
            Self::Play { rect, color } => {
                let inside = x >= rect.left
                    && play_width(rect, y).is_some_and(|width| x - rect.left < width);
                (if inside { 15 } else { 0 }, color)
            }
            Self::Icon {
                icon,
                left,
                top,
                color,
            } => return icon.pixel(x, y, left, top, look.ink(color), under),
        };
        (coverage > 0).then(|| blend(look.ink(color), under, coverage))
    }
}

type Part<'a> = (&'a [Shape], &'a [Run]);

fn paint((shapes, runs): Part<'_>, row: &mut Row) {
    for shape in shapes {
        shape.paint(row);
    }
    for run in runs {
        run.paint(row);
    }
}

#[cfg(test)]
fn pixel((shapes, runs): Part<'_>, look: Look, x: usize, y: usize, background: Shown) -> Shown {
    let mut color = background;
    for shape in shapes {
        if let Some(pixel) = shape.pixel(look, x, y, color) {
            color = pixel;
        }
    }
    for run in runs {
        if let Some(pixel) = run.pixel(look, x, y, color) {
            color = pixel;
        }
    }
    color
}

/// The columns a play triangle covers on row `y`: none outside it, at
/// least one inside.
fn play_width(rect: Rect, y: usize) -> Option<usize> {
    let (rows, wide) = (rect.height() as i32, rect.width() as i32);
    let gy = y.checked_sub(rect.top)? as i32;
    (gy < rows).then(|| ((wide * (rows - (2 * gy - (rows - 1)).abs()) / rows) as usize).max(1))
}

fn paint_bar(row: &mut Row, rect: Rect, radius: usize, color: u16) {
    if !(rect.top..rect.bottom).contains(&row.y()) {
        return;
    }
    let radius = fitted(rect, radius);
    let color = row.ink(color);
    paint_rounded_over(row, rect, radius, 0, |x, y, under| {
        let coverage = rounded(rect, radius, x, y);
        (coverage > 0).then(|| blend(color, under, coverage))
    })
}

/// A corner radius no larger than half the shape's shorter side.
const fn fitted(rect: Rect, radius: usize) -> usize {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let half = if width < height { width } else { height } / 2;
    if radius < half { radius } else { half }
}

/// Runs and shapes in paint order: shapes first, then text over them.
#[derive(Clone, Default, Hash)]
pub struct Canvas {
    pub shapes: Vec<Shape, 24>,
    pub runs: Vec<Run, 28>,
    cover: Option<(u8, u8)>,
    /// Shapes and runs that found the canvas full: the layout check fails a
    /// frame that lost any, where firmware quietly paints without them.
    #[cfg(any(test, feature = "invariants"))]
    dropped: usize,
}

impl Canvas {
    pub const EMPTY: Self = Self {
        shapes: Vec::new(),
        runs: Vec::new(),
        cover: None,
        #[cfg(any(test, feature = "invariants"))]
        dropped: 0,
    };

    pub fn start_cover(&mut self, (shapes, runs): (usize, usize)) {
        self.shapes.truncate(self.shapes.capacity() - shapes);
        self.runs.truncate(self.runs.capacity() - runs);
        self.cover = Some((self.shapes.len() as u8, self.runs.len() as u8));
    }

    fn split(&self) -> (Part<'_>, Part<'_>) {
        let (shapes, runs) = self
            .cover
            .map_or((self.shapes.len(), self.runs.len()), |(s, r)| {
                (usize::from(s), usize::from(r))
            });
        let (under_shapes, over_shapes) = self.shapes.split_at(shapes);
        let (under_runs, over_runs) = self.runs.split_at(runs);
        ((under_shapes, under_runs), (over_shapes, over_runs))
    }

    pub fn shape(&mut self, shape: Shape) {
        if self.shapes.push(shape).is_err() {
            self.drop_one();
        }
    }

    /// Place `run`; returns it for neighbours to measure from.
    pub fn run(&mut self, run: Run) -> Run {
        if self.runs.push(run).is_err() {
            self.drop_one();
        }
        run
    }

    #[cfg(any(test, feature = "invariants"))]
    fn drop_one(&mut self) {
        self.dropped += 1;
    }

    #[cfg(not(any(test, feature = "invariants")))]
    fn drop_one(&mut self) {}

    /// Fail a canvas that lost a shape or a run to its capacity.
    #[cfg(any(test, feature = "invariants"))]
    #[track_caller]
    pub fn assert_kept(&self, what: impl core::fmt::Display) {
        assert!(
            self.dropped == 0,
            "{what}: {} shapes or runs found the canvas full",
            self.dropped
        );
    }

    /// `text` in the largest of `faces` that fits `width`, placed by `align`
    /// at `x` with its capitals ending on `base`.
    #[allow(clippy::too_many_arguments)]
    pub fn fit(
        &mut self,
        faces: &[&'static Face],
        text: &[u8],
        width: usize,
        x: usize,
        base: usize,
        align: Align,
        color: u16,
    ) -> Run {
        self.run(on_baseline(
            Line::fit(faces, text, width),
            x,
            base,
            align,
            color,
        ))
    }

    /// A caption in `SECONDARY` whose capitals end on `base`, cut to `width`,
    /// in the caption face of `at`'s step.
    pub fn caption(
        &mut self,
        at: &Scaled,
        text: &[u8],
        width: usize,
        x: usize,
        base: usize,
        align: Align,
    ) -> Run {
        let face = at.face(FaceRole::Caption);
        self.fit(&[face], text, width, x, base, align, SECONDARY)
    }

    pub fn paint(&self, row: &mut Row) {
        paint(self.split().0, row);
    }

    pub fn paint_cover(&self, row: &mut Row) {
        paint(self.split().1, row);
    }

    pub fn parts(&self, parts: &mut Parts) {
        for shape in &self.shapes {
            let bounds = shape.bounds();
            parts.add(bounds.top..bounds.bottom, shape);
        }
        for run in &self.runs {
            parts.add(run.top..run.bottom(), run);
        }
    }

    /// Drop what `rows` covers, down to the first row it leaves clear, so
    /// nothing shows cut off below it or stranded off its panel.
    pub fn hide(&mut self, rows: core::ops::Range<usize>) {
        let mut cut = rows.end;
        while let Some(bottom) = self
            .spans()
            .filter(|&(top, bottom)| top < cut && cut < bottom)
            .map(|(_, bottom)| bottom)
            .max()
        {
            cut = bottom;
        }
        let shown = |top: usize, bottom: usize| bottom <= rows.start || cut <= top;
        self.shapes.retain(|shape| {
            let bounds = shape.bounds();
            shown(bounds.top, bounds.bottom)
        });
        self.runs.retain(|run| shown(run.top, run.bottom()));
    }

    /// Drop the runs that cross `rows` and the shapes wholly inside them;
    /// a shape that reaches past them keeps showing where nothing covers it.
    pub fn hide_words(&mut self, rows: core::ops::Range<usize>) {
        let clear = |top: usize, bottom: usize| bottom <= rows.start || rows.end <= top;
        self.runs.retain(|run| clear(run.top, run.bottom()));
        self.shapes.retain(|shape| {
            let bounds = shape.bounds();
            bounds.top < rows.start || rows.end < bounds.bottom
        });
    }

    fn spans(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let shapes = self.shapes.iter().map(|shape| shape.bounds());
        let shapes = shapes.map(|bounds| (bounds.top, bounds.bottom));
        shapes.chain(self.runs.iter().map(|run| (run.top, run.bottom())))
    }

    #[cfg(test)]
    pub fn pixel(&self, look: Look, x: usize, y: usize, background: Shown) -> Shown {
        pixel(self.split().0, look, x, y, background)
    }

    #[cfg(test)]
    pub fn cover_pixel(&self, look: Look, x: usize, y: usize, background: Shown) -> Shown {
        pixel(self.split().1, look, x, y, background)
    }

    /// Whether some run reads exactly `text`.
    #[cfg(test)]
    pub fn shows(&self, text: &str) -> bool {
        self.runs
            .iter()
            .any(|run| run.line.text() == text.as_bytes())
    }

    /// The run that reads exactly `text`.
    #[cfg(test)]
    pub fn find(&self, text: &str) -> Option<&Run> {
        self.runs
            .iter()
            .find(|run| run.line.text() == text.as_bytes())
    }

    /// Runs whole, inside `bounds`, apart from each other and from bars
    /// (but for a label wholly on one);
    /// every shape inside `bounds`; and each of `want` shown whole.
    #[cfg(test)]
    #[track_caller]
    pub fn assert_laid_out(&self, bounds: Rect, want: &[&str], what: &str) {
        self.assert_laid_out_at(&super::scaled::BASE, bounds, want, what);
    }

    /// `assert_laid_out`, with the word gap at `at`'s step.
    #[cfg(test)]
    #[track_caller]
    pub fn assert_laid_out_at(&self, at: &Scaled, bounds: Rect, want: &[&str], what: &str) {
        use super::chrome::layout::{assert_apart_at, assert_shown, text};
        self.assert_kept(what);
        let texts: std::vec::Vec<_> = self.runs.iter().filter_map(text).collect();
        assert_apart_at(at, &texts, bounds, what);
        // Bars may touch each other (a chart), so check each against text.
        for shape in &self.shapes {
            let b = shape.bounds();
            let inside = b.left >= bounds.left
                && b.top >= bounds.top
                && b.right <= bounds.right
                && b.bottom <= bounds.bottom;
            assert!(inside, "{what}: {shape:?} leaves {bounds:?}");
            if matches!(shape, Shape::Bar { .. } | Shape::Icon { .. }) {
                // A label on a bar, such as a zone chip's, sits wholly inside it.
                let on = |r: &Rect| {
                    r.left > b.left && r.right < b.right && r.top > b.top && r.bottom < b.bottom
                };
                let mut both = std::vec![(std::string::String::from("bar"), b)];
                both.extend(texts.iter().filter(|(_, r)| !on(r)).cloned());
                assert_apart_at(at, &both, bounds, what);
            }
        }
        assert_shown(&texts, want, what);
    }
}

/// A run whose capitals end on `base`.
pub fn on_baseline(line: Line<24>, x: usize, base: usize, align: Align, color: u16) -> Run {
    let face = line.face();
    let top = base - face.fit_height();
    Label::new(line, x, top, align, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{BLACK, POWER, WHITE};
    use crate::screens::look::look;

    #[test]
    fn shape_edges_blend_over_the_shapes_beneath() {
        let fill = Rect {
            left: 0,
            top: 0,
            right: 40,
            bottom: 40,
        };
        let mut canvas = Canvas::default();
        canvas.shape(Shape::Bar {
            rect: fill,
            radius: 0,
            color: POWER,
        });
        canvas.shape(Shape::Disc {
            cx: 80,
            cy: 80,
            radius: 12,
            color: WHITE,
        });
        canvas.shape(Shape::Bar {
            rect: Rect {
                left: 4,
                top: 2,
                right: 12,
                bottom: 4,
            },
            radius: 1,
            color: WHITE,
        });
        for light in [false, true] {
            assert_edges_blend_over_the_shapes_beneath(&canvas, look(light));
        }
    }

    fn assert_edges_blend_over_the_shapes_beneath(canvas: &Canvas, look: Look) {
        let (black, white, power) = (look.ink(BLACK), look.ink(WHITE), look.ink(POWER));
        let mut edges = 0;
        for y in 0..40 {
            let mut pixels = [black.0; 40];
            canvas.paint(&mut Row::new(y, &mut pixels).looked(look));
            for (x, &painted) in pixels.iter().enumerate() {
                let painted = Shown(painted);
                assert_eq!(painted, canvas.pixel(look, x, y, black), "({x}, {y})");
                let coverage = canvas.shapes[1..]
                    .iter()
                    .map(|shape| match *shape {
                        Shape::Disc { cx, cy, radius, .. } => disc(x, y, cx, cy, radius),
                        Shape::Bar { rect, radius, .. } => {
                            rounded(rect, fitted(rect, radius), x, y)
                        }
                        Shape::Panel { .. }
                        | Shape::Play { .. }
                        | Shape::Split { .. }
                        | Shape::Ring { .. }
                        | Shape::Icon { .. } => 0,
                    })
                    .max()
                    .unwrap_or(0);
                if (1..15).contains(&coverage) {
                    edges += 1;
                    assert_eq!(painted, blend(white, power, coverage), "({x}, {y})");
                    assert_ne!(painted, blend(white, black, coverage), "({x}, {y})");
                }
            }
        }
        assert!(edges > 8, "the disc and the cap have edges");
    }
}
