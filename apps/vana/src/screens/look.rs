//! The light look: every screen paints with the dark palette, and a light
//! look maps each painted row through `light` on its way to the panel.
use super::chrome::{BLACK, rgb};

const fn channels(pixel: u16) -> (i32, i32, i32) {
    let (r, g, b) = (
        (pixel >> 11) as i32,
        (pixel >> 5 & 0x3f) as i32,
        (pixel & 0x1f) as i32,
    );
    (r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2)
}

/// Weighted brightness, 0 to 255.
const fn luma(pixel: u16) -> i32 {
    let (r, g, b) = channels(pixel);
    (2 * r + 5 * g + b) / 8
}

/// `pixel` with its luminance inverted and its hue kept: greys swap ends
/// (black and white trade places) and bright colours turn dark enough to read
/// on white. The shift is affine before clamping, so anti-aliased blends stay
/// blends.
pub const fn light(pixel: u16) -> u16 {
    let (r, g, b) = channels(pixel);
    let shift = 255 - 2 * luma(pixel);
    rgb(clamp(r + shift), clamp(g + shift), clamp(b + shift))
}

const fn clamp(channel: i32) -> u8 {
    if channel < 0 {
        0
    } else if channel > 255 {
        255
    } else {
        channel as u8
    }
}

/// Map a painted row to the light look. Rows are long runs of one colour, so
/// the last answer is reused while the pixel repeats.
pub fn apply(pixels: &mut [u16]) {
    let (mut from, mut to) = (BLACK, light(BLACK));
    for pixel in pixels {
        if *pixel != from {
            from = *pixel;
            to = light(from);
        }
        *pixel = to;
    }
}

#[cfg(test)]
mod tests {
    use super::super::chrome::*;
    use super::*;

    #[test]
    fn black_and_white_trade_places() {
        assert_eq!(light(BLACK), WHITE);
        assert_eq!(light(WHITE), BLACK);
    }

    #[test]
    fn greys_invert_and_keep_their_order() {
        let greys = [TERTIARY, SECONDARY, MUTED, CARD, RAISED];
        for grey in greys {
            let (r, g, b) = channels(light(grey));
            assert!(
                r.abs_diff(g) <= 12 && g.abs_diff(b) <= 12,
                "{grey:04x} stays grey"
            );
            assert!(luma(light(grey)).abs_diff(255 - luma(grey)) <= 4);
        }
        assert!(luma(light(CARD)) > luma(light(RAISED)));
        assert!(luma(light(TERTIARY)) > luma(light(SECONDARY)));
    }

    #[test]
    fn every_colour_reads_on_the_light_background() {
        let colours = [
            ACCENT, GREEN, AMBER, RED, SPEED, POWER, HEART, CADENCE, GRADE,
        ]
        .into_iter()
        .chain(ZONES);
        for colour in colours {
            let mapped = light(colour);
            assert!(luma(mapped) <= 160, "{colour:04x} maps to {mapped:04x}");
            assert!(luma(light(BLACK)) - luma(mapped) >= 95);
            assert_ne!(mapped, light(BLACK));
        }
    }

    #[test]
    fn a_row_maps_pixel_by_pixel() {
        let mut row = [BLACK, BLACK, WHITE, POWER, POWER, BLACK];
        apply(&mut row);
        let expect = [BLACK, BLACK, WHITE, POWER, POWER, BLACK].map(light);
        assert_eq!(row, expect);
    }
}
