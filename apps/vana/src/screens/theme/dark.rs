//! Night: main's palette, numbers and titles in their metric colours.
use super::{Theme, rgb};

const GREY: u16 = rgb(0x8e, 0x8e, 0x93);
const BLUE: u16 = rgb(0x0a, 0x84, 0xff);
const GREEN: u16 = rgb(0x30, 0xd1, 0x58);
const YELLOW: u16 = rgb(0xff, 0xd6, 0x0a);
const ORANGE: u16 = rgb(0xff, 0x9f, 0x0a);
const RED: u16 = rgb(0xff, 0x45, 0x3a);
const PURPLE: u16 = rgb(0xbf, 0x5a, 0xf2);
const ZONES: [u16; 7] = [GREY, BLUE, GREEN, YELLOW, ORANGE, RED, PURPLE];

pub const DARK: Theme = Theme {
    bg: rgb(0x00, 0x00, 0x00),
    text: rgb(0xff, 0xff, 0xff),
    sub: GREY,
    rule: rgb(0x3a, 0x3a, 0x3c),
    card: rgb(0x1c, 0x1c, 0x1e),
    raised: rgb(0x2c, 0x2c, 0x2e),
    off: rgb(0x48, 0x48, 0x4a),
    focus: BLUE,
    on_fill: rgb(0x00, 0x00, 0x00),
    on_tone: rgb(0xff, 0xff, 0xff),
    speed: rgb(0x64, 0xd2, 0xff),
    power: YELLOW,
    heart: rgb(0xff, 0x37, 0x5f),
    cadence: GREEN,
    grade: ORANGE,
    good: GREEN,
    warn: ORANGE,
    bad: RED,
    info: BLUE,
    zone: ZONES,
    zone_fill: ZONES,
};
