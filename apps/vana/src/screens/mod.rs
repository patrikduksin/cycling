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

#[cfg(test)]
mod tests {
    use super::rounded;

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
}
