use crate::screens::chrome::{Row, WIDTH, blend};

pub type Point = (f32, f32);

/// A segment in screen pixels, with what its rows need worked out once:
/// the device has no divide or square root instruction, and a row is painted
/// every frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stroke {
    pub a: Point,
    pub b: Point,
    length: f32,
    /// The unit direction, its reciprocals, and the inverse squared length.
    along: Point,
    per_along: Point,
    per_length_squared: f32,
}

impl Stroke {
    pub fn new(a: Point, b: Point) -> Self {
        let squared = (b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1);
        let length = root(squared);
        if length <= 0.0 {
            return Self {
                a,
                b,
                ..Self::default()
            };
        }
        let along = ((b.0 - a.0) / length, (b.1 - a.1) / length);
        let reciprocal = |k: f32| if k.abs() < 1e-6 { 0.0 } else { 1.0 / k };
        Self {
            a,
            b,
            length,
            along,
            per_along: (reciprocal(along.0), reciprocal(along.1)),
            per_length_squared: 1.0 / squared,
        }
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
                let chord = root(chord);
                left = left.min(cx - chord);
                right = right.max(cx + chord);
            }
        }
        if self.length > 0.0 {
            let ((tx, ty), (per_tx, per_ty)) = (self.along, self.per_along);
            let v = y - self.a.1;
            let side = solve_interval(-ty, -per_ty, tx * v, -radius, radius);
            let along = solve_interval(tx, per_tx, ty * v, 0.0, self.length);
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

/// The `u` for which `low <= k u + c <= high`, given `per_k`, `1 / k` (0
/// when `k` is 0).
fn solve_interval(k: f32, per_k: f32, c: f32, low: f32, high: f32) -> Option<(f32, f32)> {
    if k.abs() < 1e-6 {
        return (low..=high)
            .contains(&c)
            .then_some((f32::NEG_INFINITY, f32::INFINITY));
    }
    let (p, q) = ((low - c) * per_k, (high - c) * per_k);
    Some((p.min(q), p.max(q)))
}

/// The square root by multiplies only: a reciprocal-root estimate from the
/// float's bits refined by three Newton steps, within a few ulps of
/// `sqrtf`, which on the device is a slow bit-by-bit loop.
fn root(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut r = f32::from_bits(0x5f37_59df - (x.to_bits() >> 1));
    for _ in 0..3 {
        r *= 1.5 - 0.5 * x * r * r;
    }
    x * r
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
    let t = (((px - a.0) * dx + (py - a.1) * dy) * stroke.per_length_squared).clamp(0.0, 1.0);
    let (qx, qy) = (a.0 + t * dx - px, a.1 + t * dy - py);
    let reach = half + 0.5;
    let near = qx * qx + qy * qy;
    if near >= reach * reach {
        return 0;
    }
    ((reach - root(near)).clamp(0.0, 1.0) * 15.0 + 0.5) as u8
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
            let per_k = if k.abs() < 1e-6 { 0.0 } else { 1.0 / k };
            let (from, to) = solve_interval(k, per_k, c, 0.0, f32::INFINITY)?;
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

/// A segment with square ends: every point within `half` of its line and
/// at most `half` past either end. On a row it is the overlap of two slabs,
/// each an interval that moves linearly down the rows, so a row costs a few
/// multiply-adds and no square root.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bar {
    top: f32,
    bottom: f32,
    slabs: [Slab; 2],
}

/// The columns `centre + slope * y ± half` of the row through `y`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Slab {
    centre: f32,
    slope: f32,
    half: f32,
}

impl Slab {
    /// The points within `half` of the line through `c` normal to `k`.
    fn new(k: Point, c: Point, half: f32) -> Self {
        if k.0.abs() < 1e-4 {
            return Self {
                centre: 0.0,
                slope: 0.0,
                half: f32::INFINITY,
            };
        }
        let per = 1.0 / k.0;
        let ratio = k.1 * per;
        Self {
            centre: c.0 + ratio * c.1,
            slope: -ratio,
            half: half * per.abs(),
        }
    }
}

impl Bar {
    pub fn new(a: Point, b: Point, half: f32) -> Self {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = root(dx * dx + dy * dy);
        let u = if length > 1e-3 {
            let per = 1.0 / length;
            (dx * per, dy * per)
        } else {
            (1.0, 0.0)
        };
        let c = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let along = length / 2.0 + half;
        let reach = u.1.abs() * along + u.0.abs() * half;
        Self {
            top: c.1 - reach,
            bottom: c.1 + reach,
            slabs: [Slab::new((-u.1, u.0), c, half), Slab::new(u, c, along)],
        }
    }

    /// Where the row through `y` crosses the bar.
    fn across(&self, y: f32) -> Option<(f32, f32)> {
        if y < self.top || y > self.bottom {
            return None;
        }
        let [p, q] = self.slabs;
        let (pc, qc) = (p.centre + p.slope * y, q.centre + q.slope * y);
        let left = (pc - p.half).max(qc - q.half);
        let right = (pc + p.half).min(qc + q.half);
        (left < right).then_some((left, right))
    }

    /// How much of pixel (`x`, `y`) the bar covers, 0 to 1, by the row
    /// through its centre.
    #[cfg(test)]
    pub fn share(&self, x: usize, y: usize) -> f32 {
        self.across(y as f32 + 0.5).map_or(0.0, |(left, right)| {
            (right.min(x as f32 + 1.0) - left.max(x as f32)).clamp(0.0, 1.0)
        })
    }
}

/// How much of a pixel a road covers, in the layer's four steps.
fn quarter_steps(share: f32) -> u8 {
    (share * 3.0 + 0.5) as u8
}

/// The roads of a fill, rasterised once and painted from a table each
/// frame. Every pixel of `ROWS` rows from `top` holds a nibble: two bits of
/// how much a minor road covers it and two of a major road, the largest of
/// each class's bars, so a frame costs one lookup per pixel whatever the
/// roads, and each bar is worked out once a fill instead of every frame.
/// All zero bytes are an empty layer.
#[derive(Clone)]
pub struct Layer<const ROWS: usize> {
    top: usize,
    pairs: [[u8; WIDTH / 2]; ROWS],
}

/// A road's class on the layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Minor,
    Major,
}

impl Class {
    const fn shift(self) -> u32 {
        match self {
            Self::Minor => 0,
            Self::Major => 2,
        }
    }
}

/// The two pixels of each layer byte: per nibble the ground, a minor road
/// blended over it, then a major road over that.
pub fn pairs(ground: u16, minor: u16, major: u16) -> [[u16; 2]; 256] {
    let colour = |nibble: u8| {
        let step = |shift: u32| (nibble >> shift & 3) * 5;
        blend(major, blend(minor, ground, step(0)), step(2))
    };
    core::array::from_fn(|byte| [colour(byte as u8 & 0xf), colour(byte as u8 >> 4)])
}

impl<const ROWS: usize> Layer<ROWS> {
    pub fn new(top: usize) -> Self {
        Self {
            top,
            pairs: [[0; WIDTH / 2]; ROWS],
        }
    }

    pub fn clear(&mut self, top: usize) {
        self.top = top;
        for pairs in &mut self.pairs {
            pairs.fill(0);
        }
    }

    #[cfg(test)]
    pub fn nibble(&self, x: usize, y: usize) -> u8 {
        let Some(pairs) = y.checked_sub(self.top).and_then(|r| self.pairs.get(r)) else {
            return 0;
        };
        pairs[x / 2] >> (4 * (x % 2)) & 0xf
    }

    /// Add a bar of `class`: each pixel keeps the most any bar of the class
    /// covers it.
    pub fn add(&mut self, bar: &Bar, class: Class) {
        let first = (bar.top - 0.5).max(self.top as f32);
        let last = (bar.bottom - 0.5).min((self.top + ROWS) as f32 - 1.0);
        if first > last {
            return;
        }
        let shift = class.shift();
        for y in libm::ceilf(first) as usize..=last as usize {
            let Some((left, right)) = bar.across(y as f32 + 0.5) else {
                continue;
            };
            let (left, right) = (left.max(0.0), right.min(WIDTH as f32));
            if left >= right {
                continue;
            }
            let pairs = &mut self.pairs[y - self.top];
            let (first, last) = (left as usize, libm::ceilf(right) as usize - 1);
            let mut raise = |x: usize, steps: u8| {
                let at = 4 * (x % 2) as u32 + shift;
                let pair = &mut pairs[x / 2];
                if steps > (*pair >> at & 3) {
                    *pair = (*pair & !(3 << at)) | steps << at;
                }
            };
            let share = |x: usize| right.min(x as f32 + 1.0) - left.max(x as f32);
            raise(first, quarter_steps(share(first)));
            if last > first {
                for x in first + 1..last {
                    raise(x, 3);
                }
                raise(last, quarter_steps(share(last)));
            }
        }
    }

    /// Paint `row` from `pairs`: every pixel in the layer's rows, the
    /// ground too, two at a time.
    pub fn paint(&self, row: &mut Row, pairs: &[[u16; 2]; 256]) {
        let Some(bytes) = row
            .y()
            .checked_sub(self.top)
            .and_then(|r| self.pairs.get(r))
        else {
            return;
        };
        let clip = row.clip();
        let (first, end) = (clip.start, clip.end.min(WIDTH));
        if first >= end {
            return;
        }
        let pixels = row.pixels_mut();
        let (whole_from, whole_to) = (first.div_ceil(2), end / 2);
        if whole_from < whole_to {
            let (chunks, _) = pixels[2 * whole_from..2 * whole_to].as_chunks_mut::<2>();
            for (chunk, &byte) in chunks.iter_mut().zip(&bytes[whole_from..whole_to]) {
                *chunk = pairs[usize::from(byte)];
            }
        }
        if first % 2 == 1 {
            pixels[first] = pairs[usize::from(bytes[first / 2])][1];
        }
        if end % 2 == 1 && end - 1 > first {
            pixels[end - 1] = pairs[usize::from(bytes[end / 2])][0];
        }
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
    fn a_layer_holds_each_class_s_largest_share_of_a_pixel_in_four_steps() {
        let mut random = SeededStrokes(11);
        let mut layer = std::boxed::Box::new(Layer::<40>::new(5));
        for case in 0..300 {
            let half = [2.5, 4.5][case % 2];
            let class = [Class::Minor, Class::Major][case / 2 % 2];
            let bars: std::vec::Vec<Bar> = (0..3)
                .map(|_| {
                    let (a, b) = match case % 4 {
                        0 => {
                            let a = random.point();
                            (a, (a.0, a.1 + random.next() * 30.0))
                        }
                        1 => {
                            let a = random.point();
                            (a, (a.0 + random.next() * 90.0, a.1))
                        }
                        2 => {
                            let a = random.point();
                            (a, a)
                        }
                        _ => (random.point(), random.point()),
                    };
                    Bar::new(a, b, half)
                })
                .collect();
            layer.clear(5);
            for bar in &bars {
                layer.add(bar, class);
            }
            for y in 0..50 {
                for x in 0..WIDTH {
                    let share = bars.iter().map(|bar| bar.share(x, y)).fold(0.0, f32::max);
                    let want = if (5..45).contains(&y) {
                        quarter_steps(share)
                    } else {
                        0
                    };
                    let got = layer.nibble(x, y) >> class.shift() & 3;
                    assert_eq!(got, want, "case {case} at {x},{y}: share {share}");
                }
            }
        }
    }

    #[test]
    fn a_bar_ends_square_and_a_diagonal_one_spans_its_slant() {
        let bar = Bar::new((100.0, 10.0), (100.0, 40.0), 4.5);
        let row = |y: usize| {
            (0..WIDTH)
                .map(|x| quarter_steps(bar.share(x, y)))
                .collect::<std::vec::Vec<_>>()
        };
        assert_eq!(&row(25)[95..106], &[2, 3, 3, 3, 3, 3, 3, 3, 3, 2, 0][..]);
        assert_eq!(
            row(44)[95..106],
            row(25)[95..106],
            "square past the end, not round"
        );
        assert!(row(46).iter().all(|&c| c == 0));
        let diagonal = Bar::new((0.0, 0.0), (30.0, 30.0), 2.5);
        let width = (0..WIDTH).filter(|&x| diagonal.share(x, 15) > 0.0).count();
        assert!(
            (7..=9).contains(&width),
            "a 5 px road at 45° spans 5√2 columns: {width}"
        );
    }

    fn pixels_of(layer: &Layer<20>, y: usize, pairs: &[[u16; 2]; 256]) -> [u16; WIDTH] {
        core::array::from_fn(|x| pairs[usize::from(layer.nibble(x, y))][0])
    }

    #[test]
    fn a_layer_paints_its_ground_and_roads_from_the_palette() {
        let mut layer = std::boxed::Box::new(Layer::<20>::new(20));
        layer.add(&Bar::new((50.0, 0.0), (50.0, 40.0), 2.5), Class::Minor);
        layer.add(&Bar::new((0.0, 25.0), (240.0, 25.0), 4.5), Class::Major);
        let pairs = pairs(0x1082, 0x2945, 0x4229);
        let mut pixels = [0x1234u16; WIDTH];
        layer.paint(&mut Row::new(35, &mut pixels), &pairs);
        assert_eq!((pixels[10], pixels[50]), (0x1082, 0x2945));
        assert_eq!(
            pixels[47], pairs[2][0],
            "an edge half covered rounds to two steps"
        );
        assert_eq!(pixels, pixels_of(&layer, 35, &pairs));
        let mut clipped = [0x1234u16; WIDTH];
        layer.paint(&mut Row::new(35, &mut clipped).within(29..48), &pairs);
        assert_eq!(clipped[28], 0x1234, "left of the clip is left alone");
        assert_eq!(&clipped[29..48], &pixels[29..48]);
        layer.paint(&mut Row::new(25, &mut pixels), &pairs);
        assert_eq!(pixels[50], 0x4229, "a major road over a minor one");
        let mut outside = [0x1234u16; WIDTH];
        layer.paint(&mut Row::new(40, &mut outside), &pairs);
        assert!(
            outside.iter().all(|&p| p == 0x1234),
            "rows past the layer are left alone"
        );
    }

    #[test]
    fn the_multiply_only_root_matches_the_square_root() {
        let mut x = 1e-4f32;
        while x < 1e5 {
            let (got, want) = (root(x), libm::sqrtf(x));
            assert!((got - want).abs() <= want * 1e-6, "{x}: {got} for {want}");
            x *= 1.013;
        }
        assert_eq!(root(0.0), 0.0);
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
