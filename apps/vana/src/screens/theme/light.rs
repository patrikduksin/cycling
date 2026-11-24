//! Day: words and numbers in black on white, to read in direct sun; colour
//! stays on bars, charts, fills and state marks.
use super::{DARK, Theme, rgb};

const BLACK: u16 = rgb(0x00, 0x00, 0x00);
const RED: u16 = rgb(0xd7, 0x00, 0x15);
const BLUE: u16 = rgb(0x00, 0x69, 0xe7);

pub const LIGHT: Theme = Theme {
    bg: rgb(0xff, 0xff, 0xff),
    text: BLACK,
    sub: rgb(0x3a, 0x3a, 0x3c),
    rule: rgb(0xc7, 0xc7, 0xcc),
    card: rgb(0xf2, 0xf2, 0xf7),
    raised: rgb(0xe5, 0xe5, 0xea),
    off: rgb(0xd1, 0xd1, 0xd6),
    focus: BLUE,
    on_fill: BLACK,
    on_tone: rgb(0xff, 0xff, 0xff),
    speed: BLACK,
    power: BLACK,
    heart: BLACK,
    cadence: BLACK,
    grade: BLACK,
    good: rgb(0x1f, 0x8a, 0x3b),
    warn: rgb(0xb5, 0x55, 0x00),
    bad: RED,
    info: BLUE,
    zone: [
        rgb(0x8e, 0x8e, 0x93),
        BLUE,
        rgb(0x00, 0x92, 0x18),
        rgb(0xf2, 0xc2, 0x00),
        rgb(0xb5, 0x55, 0x00),
        RED,
        rgb(0x89, 0x44, 0xab),
    ],
    zone_fill: DARK.zone_fill,
};
