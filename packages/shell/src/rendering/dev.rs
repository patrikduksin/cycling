//! Development identity, rendered directly into the shell's display strips.
use super::text::text;

/// Three torn frames followed by a long, readable hold. No redraw during the hold.
pub(crate) fn frame(now: u64) -> u8 {
    match now % 4800 {
        3600..3800 => 1,
        3800..4000 => 2,
        4000..4200 => 3,
        _ => 0,
    }
}

pub(crate) fn pixel(x: usize, y: usize, frame: u8) -> u16 {
    const INK: u16 = 0x0843;
    const PAPER: u16 = 0xffb9;
    const CYAN: u16 = 0x07ff;
    const PINK: u16 = 0xf84e;
    const YELLOW: u16 = 0xff60;

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
