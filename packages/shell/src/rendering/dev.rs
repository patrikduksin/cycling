//! Development identity, rendered directly into the shell's display strips.
use super::row::Row;
use super::text::text;

const INK: u16 = 0x0843;
const PAPER: u16 = 0xffb9;
const CYAN: u16 = 0x07ff;
const PINK: u16 = 0xf84e;
const YELLOW: u16 = 0xff60;

/// Three torn frames followed by a long, readable hold. No redraw during the hold.
pub(crate) fn frame(now: u64) -> u8 {
    match now % 4800 {
        3600..3800 => 1,
        3800..4000 => 2,
        4000..4200 => 3,
        _ => 0,
    }
}

/// Paint one logical row; identical to `pixel` across the 240 x 320 canvas.
pub(crate) fn paint(row: &mut Row, frame: u8) {
    let y = row.y();
    row.fill(INK);
    if y == 308 {
        for k in 1..19 {
            row.span(k * 12..k * 12 + 12, if k % 3 == 0 { PINK } else { 0x2948 });
        }
    }
    if (205..234).contains(&y) && y.is_multiple_of(6) {
        for x in (0..60).step_by(6) {
            row.set(x, 0x0290);
        }
    }
    if y < 78 && y.is_multiple_of(7) {
        let first = 189 + (7 - (189 + y / 2) % 7) % 7;
        for x in (first..row.width()).step_by(7) {
            row.set(x, 0x4810);
        }
    }
    if frame != 0 {
        let color = if frame == 2 { CYAN } else { PINK };
        if y == 86 {
            row.span(142..225, color);
        }
        if y == 168 {
            row.span(12..76, color);
        }
    }
    // Speed lines: each row of a stepped rule is one run of its run length.
    if y >= 70 {
        let start = (y - 70) * 8;
        row.span(start..(start + 8).min(115), CYAN);
    }
    if y <= 216 {
        let start = (216 - y) * 12;
        row.span(start.max(161)..start + 12, CYAN);
    }
    if y >= 65 {
        let start = (y - 65) * 8;
        row.span(start..(start + 8).min(183), PINK);
    }
    if y <= 212 {
        let start = (212 - y) * 12;
        row.span(start.max(66)..start + 12, PINK);
    }
    // The crooked label's rows slant with x, so it keeps a per-pixel test.
    if (236..276).contains(&y) {
        row.each(24..216, |x| {
            let label_y = y + x / 24;
            (244..276).contains(&label_y).then(|| {
                if text(x, label_y, 49, 253, 3, b"DEV MODE") {
                    INK
                } else {
                    YELLOW
                }
            })
        });
    }
    row.text(180, 180, 2, b"NYC", CYAN);
    row.text(24, 180, 2, b"LABS, INC.", PAPER);
    if (91..163).contains(&y) {
        let shear = (162 - y) / 6;
        let tear = if frame != 0 && (y / 11 + frame as usize).is_multiple_of(3) {
            9
        } else {
            0
        };
        // `pixel` samples the type at x + 18 - shear + tear; shift the type instead.
        let shift = |left: usize| left + shear - tear - 18;
        row.text(shift(48), 103, 8, b"VANA", PINK);
        row.text(shift(42) - 5, 96, 8, b"VANA", CYAN);
        let paper = if frame == 2 && (125..130).contains(&y) {
            CYAN
        } else {
            PAPER
        };
        row.text(shift(42), 99, 8, b"VANA", paper);
    }
}

/// The per-pixel original, kept as the reference `paint` must match.
#[cfg(test)]
fn pixel(x: usize, y: usize, frame: u8) -> u16 {
    // Slanted block type, with print-registration offsets and horizontal tears.
    if (91..163).contains(&y) {
        let shear = (162 - y) / 6;
        let tear = if frame != 0 && (y / 11 + frame as usize).is_multiple_of(3) {
            9
        } else {
            0
        };
        let u = x + 18 - shear + tear;
        if text(u, y, 42, 99, 8, b"VANA") {
            return if frame == 2 && (125..130).contains(&y) {
                CYAN
            } else {
                PAPER
            };
        }
        if text(u + 5, y + 3, 42, 99, 8, b"VANA") {
            return CYAN;
        }
        if text(u, y, 48, 103, 8, b"VANA") {
            return PINK;
        }
    }
    if text(x, y, 24, 180, 2, b"LABS, INC.") {
        return PAPER;
    }
    if text(x, y, 180, 180, 2, b"NYC") {
        return CYAN;
    }
    // A slightly crooked yellow label, like tape on a printed poster.
    let label_y = y + x / 24;
    if (24..216).contains(&x) && (244..276).contains(&label_y) {
        return if text(x, label_y, 49, 253, 3, b"DEV MODE") {
            INK
        } else {
            YELLOW
        };
    }
    // Cropped comic-panel rules, speed lines and sparse halftone ink.
    if (y == 65 + x / 8 && x < 183) || (y == 212 - x / 12 && x > 65) {
        return PINK;
    }
    if (y == 70 + x / 8 && x < 115) || (y == 216 - x / 12 && x > 160) {
        return CYAN;
    }
    if frame != 0 && ((y == 86 && (142..225).contains(&x)) || (y == 168 && (12..76).contains(&x))) {
        return if frame == 2 { CYAN } else { PINK };
    }
    if x > 188 && y < 78 && (x + y / 2).is_multiple_of(7) && y.is_multiple_of(7) {
        return 0x4810;
    }
    if x < 60 && (205..234).contains(&y) && x.is_multiple_of(6) && y.is_multiple_of(6) {
        return 0x0290;
    }
    if (12..228).contains(&x) && y == 308 {
        return if (x / 12).is_multiple_of(3) {
            PINK
        } else {
            0x2948
        };
    }
    INK
}

#[cfg(test)]
mod tests {
    #[test]
    fn rows_match_the_per_pixel_reference_in_every_frame() {
        for frame in 0..4 {
            for y in 0..320 {
                let mut pixels = [0u16; 240];
                super::paint(&mut super::Row::new(y, &mut pixels), frame);
                for (x, &pixel) in pixels.iter().enumerate() {
                    assert_eq!(pixel, super::pixel(x, y, frame), "frame {frame} at {x},{y}");
                }
            }
        }
    }
}
