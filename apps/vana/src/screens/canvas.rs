//! A prepared screen: text runs and shapes laid out once per frame, then
//! painted a display row at a time.
//!
//! Screens build a `Canvas` in `prepare` from measured text (`Line::fit`)
//! and the shapes of `docs/ui.md`, so placement is decided once and painting
//! only fills spans and blends glyph runs. `pixel` is the per-pixel original
//! that tests hold `paint` to, and `assert_laid_out` is the layout sweep's
//! check.
#[cfg(test)]
use super::chrome::disc;
use super::chrome::{
    Align, CAPTION, Label, Line, Rect, Row, SECONDARY, blend, paint_disc_over, paint_rounded_over,
    rounded,
};
use firmware_shell::rendering::font::{Face, SPILL};
use heapless::Vec;

/// Text on a canvas: up to 24 characters.
pub type Run = Label<24>;

/// Columns kept clear between neighbouring runs, their spill included.
pub const GAP: usize = 8 + SPILL;

#[derive(Clone, Copy, Debug)]
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
    /// A disc around (cx, cy) of `radius`, all in quarter pixels.
    Disc {
        cx: i32,
        cy: i32,
        radius: i32,
        color: u16,
    },
}

impl Shape {
    /// The box the shape can cover.
    pub fn bounds(&self) -> Rect {
        match *self {
            Self::Panel { rect, .. } | Self::Bar { rect, .. } => rect,
            Self::Disc { cx, cy, radius, .. } => Rect {
                left: ((cx - radius - 2) / 4).max(0) as usize,
                top: ((cy - radius - 2) / 4).max(0) as usize,
                right: ((cx + radius + 2) / 4 + 1) as usize,
                bottom: ((cy + radius + 2) / 4 + 1) as usize,
            },
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
            } => {
                let radius = fitted(rect, radius);
                paint_rounded_over(row, rect, radius, 0, |x, y, under| {
                    let coverage = rounded(rect, radius, x, y);
                    (coverage > 0).then(|| blend(color, under, coverage))
                })
            }
            Self::Disc {
                cx,
                cy,
                radius,
                color,
            } => paint_disc_over(row, (cx, cy, radius), color, |_, coverage, under| {
                Some(blend(color, under, coverage))
            }),
        }
    }

    #[cfg(test)]
    fn pixel(&self, x: usize, y: usize, under: u16) -> Option<u16> {
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
            Self::Disc {
                cx,
                cy,
                radius,
                color,
            } => (disc(x, y, cx, cy, radius), color),
        };
        (coverage > 0).then(|| blend(color, under, coverage))
    }
}

/// A corner radius no larger than half the shape's shorter side.
const fn fitted(rect: Rect, radius: usize) -> usize {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let half = if width < height { width } else { height } / 2;
    if radius < half { radius } else { half }
}

/// Runs and shapes in paint order: shapes first, then text over them.
#[derive(Clone, Default)]
pub struct Canvas {
    pub shapes: Vec<Shape, 24>,
    pub runs: Vec<Run, 28>,
}

impl Canvas {
    pub fn shape(&mut self, shape: Shape) {
        let _ = self.shapes.push(shape);
    }

    /// Place `run`; returns it for neighbours to measure from.
    pub fn run(&mut self, run: Run) -> Run {
        let _ = self.runs.push(run);
        run
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

    /// A `CAPTION` in `SECONDARY` whose capitals end on `base`, cut to `width`.
    pub fn caption(
        &mut self,
        text: &[u8],
        width: usize,
        x: usize,
        base: usize,
        align: Align,
    ) -> Run {
        self.fit(&[&CAPTION], text, width, x, base, align, SECONDARY)
    }

    pub fn paint(&self, row: &mut Row) {
        for shape in &self.shapes {
            shape.paint(row);
        }
        for run in &self.runs {
            run.paint(row);
        }
    }

    /// The per-pixel original of `paint`, over `background`.
    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize, background: u16) -> u16 {
        let mut color = background;
        for shape in &self.shapes {
            if let Some(pixel) = shape.pixel(x, y, color) {
                color = pixel;
            }
        }
        for run in &self.runs {
            if let Some(pixel) = run.pixel(x, y, color) {
                color = pixel;
            }
        }
        color
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

    /// Runs whole, inside `bounds`, apart from each other and from bars;
    /// every shape inside `bounds`; and each of `want` shown whole.
    #[cfg(test)]
    #[track_caller]
    pub fn assert_laid_out(&self, bounds: Rect, want: &[&str], what: &str) {
        use super::chrome::layout::{assert_apart, assert_shown, text};
        let texts: std::vec::Vec<_> = self.runs.iter().filter_map(text).collect();
        assert_apart(&texts, bounds, what);
        // Bars may touch each other (a chart), so check each against text.
        for shape in &self.shapes {
            let b = shape.bounds();
            let inside = b.left >= bounds.left
                && b.top >= bounds.top
                && b.right <= bounds.right
                && b.bottom <= bounds.bottom;
            assert!(inside, "{what}: {shape:?} leaves {bounds:?}");
            if matches!(shape, Shape::Bar { .. }) {
                let mut both = std::vec![(std::string::String::from("bar"), b)];
                both.extend(texts.iter().cloned());
                assert_apart(&both, bounds, what);
            }
        }
        assert_shown(&texts, want, what);
    }
}

/// A run whose capitals end on `base`.
pub fn on_baseline(line: Line<24>, x: usize, base: usize, align: Align, color: u16) -> Run {
    let face = line.face();
    let top = base - face.top as usize - face.cap as usize;
    Label::new(line, x, top, align, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{BLACK, POWER, WHITE};

    /// A shape's anti-aliased edge mixes its colour with what lies under it,
    /// such as the rider's disc on the climb profile or a lap bar's cap,
    /// never with black; `paint` and `pixel` agree.
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
        let mut edges = 0;
        for y in 0..40 {
            let mut pixels = [BLACK; 40];
            canvas.paint(&mut Row::new(y, &mut pixels));
            for (x, &painted) in pixels.iter().enumerate() {
                assert_eq!(painted, canvas.pixel(x, y, BLACK), "({x}, {y})");
                let coverage = canvas.shapes[1..]
                    .iter()
                    .map(|shape| match *shape {
                        Shape::Disc { cx, cy, radius, .. } => disc(x, y, cx, cy, radius),
                        Shape::Bar { rect, radius, .. } => {
                            rounded(rect, fitted(rect, radius), x, y)
                        }
                        Shape::Panel { .. } => 0,
                    })
                    .max()
                    .unwrap_or(0);
                if (1..15).contains(&coverage) {
                    edges += 1;
                    assert_eq!(painted, blend(WHITE, POWER, coverage), "({x}, {y})");
                    assert_ne!(painted, blend(WHITE, BLACK, coverage), "({x}, {y})");
                }
            }
        }
        assert!(edges > 8, "the disc and the cap have edges");
    }
}
