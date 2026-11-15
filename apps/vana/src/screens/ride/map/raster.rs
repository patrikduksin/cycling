use crate::screens::chrome::{Row, WIDTH, blend};
use libm::sqrtf;

pub type Point = (f32, f32);

/// A segment in screen pixels, with its inverse length kept for the rows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stroke {
    pub a: Point,
    pub b: Point,
    inverse: f32,
}

impl Stroke {
    pub fn new(a: Point, b: Point) -> Self {
        let length = sqrtf((b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1));
        let inverse = if length > 0.0 { 1.0 / length } else { 0.0 };
        Self { a, b, inverse }
    }

    /// Rows `half` reaches, edges included.
    pub fn rows(&self, half: f32) -> (f32, f32) {
        let reach = half + 0.5;
        (
            self.a.1.min(self.b.1) - reach,
            self.a.1.max(self.b.1) + reach,
        )
    }

    fn row_interval(&self, radius: f32, y: f32) -> Option<(f32, f32)> {
        let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
        for (cx, cy) in [self.a, self.b] {
            let chord = radius * radius - (y - cy) * (y - cy);
            if chord >= 0.0 {
                let chord = sqrtf(chord);
                left = left.min(cx - chord);
                right = right.max(cx + chord);
            }
        }
        if self.inverse > 0.0 {
            let (tx, ty) = (
                (self.b.0 - self.a.0) * self.inverse,
                (self.b.1 - self.a.1) * self.inverse,
            );
            let length = 1.0 / self.inverse;
            let v = y - self.a.1;
            let side = solve_interval(-ty, tx * v, -radius, radius);
            let along = solve_interval(tx, ty * v, 0.0, length);
            if let (Some(side), Some(along)) = (side, along) {
                let (from, to) = (side.0.max(along.0), side.1.min(along.1));
                if from <= to {
                    left = left.min(self.a.0 + from);
                    right = right.max(self.a.0 + to);
                }
            }
        }
        (left <= right).then_some((left, right))
    }
}

fn solve_interval(k: f32, c: f32, low: f32, high: f32) -> Option<(f32, f32)> {
    if k.abs() < 1e-6 {
        return (low..=high)
            .contains(&c)
            .then_some((f32::NEG_INFINITY, f32::INFINITY));
    }
    let (p, q) = ((low - c) / k, (high - c) / k);
    Some((p.min(q), p.max(q)))
}

fn centre_columns((left, right): (f32, f32)) -> (i32, i32) {
    let first = libm::ceilf(left - 0.5);
    let last = libm::floorf(right - 0.5);
    (
        first.clamp(-1.0, WIDTH as f32) as i32,
        (last + 1.0).clamp(-1.0, WIDTH as f32) as i32,
    )
}

/// How much of pixel (`x`, `y`) a stroke of `half` covers, 0 to 15, from
/// the distance between the pixel's centre and the segment.
pub fn coverage(stroke: &Stroke, half: f32, x: usize, y: usize) -> u8 {
    let (a, b) = (stroke.a, stroke.b);
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((px - a.0) * dx + (py - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.0 + t * dx - px, a.1 + t * dy - py);
    let reach = half + 0.5;
    let near = qx * qx + qy * qy;
    if near >= reach * reach {
        return 0;
    }
    ((reach - sqrtf(near)).clamp(0.0, 1.0) * 15.0 + 0.5) as u8
}

/// A filled triangle, wound either way.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Triangle(pub [Point; 3]);

impl Triangle {
    fn edges(&self) -> [(Point, Point); 3] {
        let [p, q, r] = self.0;
        [(p, q), (q, r), (r, p)]
    }

    fn winding(&self) -> f32 {
        let [p, q, r] = self.0;
        let cross = (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0);
        if cross < 0.0 { -1.0 } else { 1.0 }
    }

    fn row_interval(&self, y: f32) -> Option<(f32, f32)> {
        let sign = self.winding();
        let (mut left, mut right) = (f32::NEG_INFINITY, f32::INFINITY);
        for (p, q) in self.edges() {
            let k = -sign * (q.1 - p.1);
            let c = sign * ((q.0 - p.0) * (y - p.1) + (q.1 - p.1) * p.0);
            let (from, to) = solve_interval(k, c, 0.0, f32::INFINITY)?;
            left = left.max(from);
            right = right.min(to);
        }
        (left <= right).then_some((left, right))
    }

    #[cfg(test)]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.row_interval(y)
            .is_some_and(|(left, right)| (left..=right).contains(&x))
    }

    #[cfg(test)]
    pub fn coverage(&self, outline: f32, x: usize, y: usize) -> u8 {
        if self.contains(x as f32 + 0.5, y as f32 + 0.5) {
            return 15;
        }
        self.edges()
            .into_iter()
            .map(|(p, q)| coverage(&Stroke::new(p, q), outline, x, y))
            .max()
            .unwrap_or(0)
    }
}

/// One row's coverage of one colour, built from strokes and painted once.
pub struct Cover {
    cells: [u8; WIDTH],
    y: usize,
    first: i32,
    end: i32,
}

impl Cover {
    pub fn new(y: usize) -> Self {
        Self {
            cells: [0; WIDTH],
            y,
            first: WIDTH as i32,
            end: 0,
        }
    }

    fn touch(&mut self, (first, end): (i32, i32)) {
        self.first = self.first.min(first.max(0));
        self.end = self.end.max(end.min(WIDTH as i32));
    }

    fn solid(&mut self, (first, end): (i32, i32)) {
        for cell in &mut self.cells[first.max(0) as usize..end.clamp(0, WIDTH as i32) as usize] {
            *cell = 15;
        }
    }

    /// Add a stroke of `half` to the row: the union, by the larger coverage.
    pub fn stroke(&mut self, stroke: &Stroke, half: f32) {
        let y = self.y as f32 + 0.5;
        let (top, bottom) = stroke.rows(half);
        if y < top || y > bottom {
            return;
        }
        let Some(outer) = stroke.row_interval(half + 0.5, y) else {
            return;
        };
        let outer = centre_columns(outer);
        if outer.0 >= outer.1 {
            return;
        }
        self.touch(outer);
        let inner = stroke
            .row_interval(half - 0.5, y)
            .map(centre_columns)
            .filter(|inner| inner.0 < inner.1)
            .unwrap_or((outer.0, outer.0));
        self.solid(inner);
        for x in (outer.0..inner.0).chain(inner.1..outer.1) {
            let Ok(x) = usize::try_from(x) else {
                continue;
            };
            if let Some(cell) = self.cells.get_mut(x)
                && *cell < 15
            {
                *cell = (*cell).max(coverage(stroke, half, x, self.y));
            }
        }
    }

    /// Add a triangle with its edges stroked by `outline`.
    pub fn triangle(&mut self, triangle: &Triangle, outline: f32) {
        let ys = triangle.0.map(|p| p.1);
        let y = self.y as f32 + 0.5;
        let reach = outline + 0.5;
        if y < ys[0].min(ys[1]).min(ys[2]) - reach || y > ys[0].max(ys[1]).max(ys[2]) + reach {
            return;
        }
        if let Some(inside) = triangle.row_interval(self.y as f32 + 0.5) {
            let inside = centre_columns(inside);
            if inside.0 < inside.1 {
                self.touch(inside);
                self.solid(inside);
            }
        }
        for (p, q) in triangle.edges() {
            self.stroke(&Stroke::new(p, q), outline);
        }
    }

    /// Paint `color` where the row is covered: solid runs as spans, edges
    /// blended over what the row holds. Then start over for the next colour.
    pub fn paint(&mut self, row: &mut Row, color: u16) {
        let (first, end) = (self.first as usize, self.end as usize);
        let cells = &self.cells;
        let mut x = first;
        while x < end {
            let start = x;
            let solid = cells[x] == 15;
            while x < end && (cells[x] == 15) == solid {
                x += 1;
            }
            if solid {
                row.span(start..x, color);
            } else {
                row.each_over(start..x, |x, under| match cells[x] {
                    0 => None,
                    c => Some(blend(color, under, c)),
                });
            }
        }
        if first < end {
            self.cells[first..end].fill(0);
        }
        (self.first, self.end) = (WIDTH as i32, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SeededStrokes(u32);

    impl SeededStrokes {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }
        fn point(&mut self) -> Point {
            (self.next() * 280.0 - 20.0, self.next() * 60.0 - 10.0)
        }
    }

    fn spans(strokes: &[Stroke], half: f32, y: usize) -> [u8; WIDTH] {
        let mut cover = Cover::new(y);
        for stroke in strokes {
            cover.stroke(stroke, half);
        }
        cover.cells
    }

    #[test]
    fn spans_cover_each_pixel_as_the_per_pixel_reference_does() {
        let mut random = SeededStrokes(7);
        let mut worst = 0;
        for case in 0..400 {
            let half = [0.5, 1.75, 2.5, 4.25, 6.0, 9.0][case % 6];
            let mut strokes = std::vec::Vec::new();
            for _ in 0..3 {
                strokes.push(match case % 5 {
                    0 => {
                        let a = random.point();
                        Stroke::new(a, (a.0, a.1 + random.next() * 30.0))
                    }
                    1 => {
                        let a = random.point();
                        Stroke::new(a, (a.0 + random.next() * 90.0, a.1 + random.next() * 0.3))
                    }
                    2 => {
                        let a = random.point();
                        Stroke::new(a, a)
                    }
                    _ => Stroke::new(random.point(), random.point()),
                });
            }
            for y in 0..40 {
                let got = spans(&strokes, half, y);
                for (x, &got) in got.iter().enumerate() {
                    let want = strokes
                        .iter()
                        .map(|s| coverage(s, half, x, y))
                        .max()
                        .unwrap_or(0);
                    let off = got.abs_diff(want);
                    assert!(off <= 1, "case {case} at {x},{y}: {got} for {want}");
                    worst = worst.max(off);
                }
            }
        }
        assert_eq!(worst, 0, "spans use the reference's own edge coverage");
    }

    #[test]
    fn a_wide_stroke_is_solid_inside_and_round_at_its_caps() {
        let stroke = Stroke::new((100.0, 10.0), (100.0, 40.0));
        let solid = |y| {
            spans(&[stroke], 9.0, y)
                .iter()
                .filter(|&&c| c == 15)
                .count()
        };
        let row = spans(&[stroke], 9.0, 25);
        assert!(row[91..109].iter().all(|&c| c == 15), "{:?}", &row[88..112]);
        assert_eq!((row[90], row[109]), (0, 0));
        assert_eq!(solid(25), 18);
        assert_eq!(solid(44), 14);
        assert_eq!(solid(49), 0);
    }

    #[test]
    fn a_triangle_fills_inside_and_strokes_its_edges() {
        let head = Triangle([(106.4, 17.1), (96.0, 25.1), (95.8, 9.4)]);
        for y in 5..30 {
            let mut cover = Cover::new(y);
            cover.triangle(&head, 0.5);
            for (x, &cell) in cover.cells.iter().enumerate().take(120).skip(80) {
                assert_eq!(cell, head.coverage(0.5, x, y), "{x},{y}");
            }
        }
        assert!(head.contains(99.0, 17.0) && !head.contains(110.0, 17.0));
    }

    #[test]
    fn painting_blends_the_edges_and_stores_the_inside() {
        let mut pixels = [0x1234u16; WIDTH];
        let mut row = Row::new(20, &mut pixels);
        let mut cover = Cover::new(20);
        cover.stroke(&Stroke::new((50.3, 0.0), (50.3, 40.0)), 6.0);
        cover.paint(&mut row, 0xffff);
        assert_eq!(pixels[50], 0xffff);
        assert_eq!(pixels[43], 0x1234);
        assert!(![0x1234, 0xffff].contains(&pixels[44]), "an edge blends");
        assert!(
            cover.cells.iter().all(|&c| c == 0),
            "ready for the next colour"
        );
    }
}
