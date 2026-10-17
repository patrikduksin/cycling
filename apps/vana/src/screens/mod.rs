pub mod ride;
pub mod sensors;
pub mod settings;
pub mod workout;

/// Whether (`x`, `y`) lies in the rectangle `left..right` × `top..bottom`
/// with corners rounded to `radius`. Integer-only, for per-pixel drawing.
pub(crate) fn rounded(
    x: usize,
    y: usize,
    (left, top, right, bottom): (usize, usize, usize, usize),
    radius: usize,
) -> bool {
    if !((left..right).contains(&x) && (top..bottom).contains(&y)) {
        return false;
    }
    // Distance into the nearest corner square, if the pixel is in one.
    let dx = (left + radius)
        .saturating_sub(x + 1)
        .max((x + radius).saturating_sub(right));
    let dy = (top + radius)
        .saturating_sub(y + 1)
        .max((y + radius).saturating_sub(bottom));
    // Sample the pixel centre against a circle of `radius` (in half pixels).
    let (dx, dy, r) = (2 * dx + 1, 2 * dy + 1, 2 * radius);
    dx == 1 || dy == 1 || dx * dx + dy * dy <= r * r + r
}

/// The columns of row `y` that `rounded` covers, as one span, for row painting.
pub(crate) fn rounded_span(
    y: usize,
    bounds: (usize, usize, usize, usize),
    radius: usize,
) -> core::ops::Range<usize> {
    let (left, top, right, bottom) = bounds;
    if !(top..bottom).contains(&y) || left >= right {
        return 0..0;
    }
    // Rows are mirror-symmetric and solid between their ends, so the first
    // covered column from the left fixes the span.
    let half = (right - left).div_ceil(2);
    (left..left + half)
        .find(|&x| rounded(x, y, bounds, radius))
        .map_or(0..0, |start| start..left + right - start)
}

#[cfg(test)]
mod tests {
    use super::{rounded, rounded_span};

    #[test]
    fn rounded_rectangles_cut_their_corners_only() {
        let bounds = (10, 20, 30, 28);
        assert!(!rounded(10, 20, bounds, 4), "corner cut");
        assert!(rounded(14, 20, bounds, 4), "top edge past the corner");
        assert!(rounded(10, 24, bounds, 4), "left edge past the corner");
        assert!(rounded(20, 24, bounds, 4));
        assert!(!rounded(29, 27, bounds, 4));
        assert!(rounded(10, 20, bounds, 0), "square when radius is zero");
        assert!(!rounded(30, 24, bounds, 4), "right edge is exclusive");
    }

    #[test]
    fn rounded_spans_cover_exactly_the_rounded_pixels() {
        for bounds in [
            (10, 20, 30, 28),
            (3, 0, 4, 9),
            (5, 5, 12, 40),
            (0, 0, 216, 72),
        ] {
            for radius in 0..14 {
                for y in 0..45 {
                    let span = rounded_span(y, bounds, radius);
                    for x in 0..240 {
                        assert_eq!(
                            span.contains(&x),
                            rounded(x, y, bounds, radius),
                            "{bounds:?} radius {radius} at {x},{y}"
                        );
                    }
                }
            }
        }
    }
}
