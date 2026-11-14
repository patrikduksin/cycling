//! The display mode: how the screen trades smoothness for battery. The rider
//! switches it by hand in Settings, and `TABLE` is the one place that says
//! what each mode does. A mode has no variants to match on: code reads the
//! fields of `policy()`, so a new mode is a new row, not a new branch.
use firmware_shell::backlight::Curve;

/// When the ride pages' numbers change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Numbers {
    /// As each reading arrives.
    OnArrival,
    /// Once a second, when the ride clock's second turns, and at once when
    /// the page, the lap or a climb changes.
    EverySecond,
}

/// What a mode does.
#[derive(Debug)]
pub struct Policy {
    pub name: &'static str,
    /// Gauges and radar markers glide to each reading, or jump to it.
    pub glides: bool,
    pub numbers: Numbers,
    /// What the Mode row says under its name.
    pub note: &'static str,
    /// The least time between two frames a glide asks for.
    pub motion_frame_ms: u64,
    /// The backlight percent for each ambient light reading while the rider
    /// leaves brightness on AUTO.
    pub light_curve: &'static Curve,
}

/// Readings are the companion's raw units: about 36 to 41 in a dark room at
/// night, 65 covered, 68 in a dim room, 470 to 1,557 under a phone torch. No
/// daylight reading is known yet, so the top knot sits well above the torch
/// and saturates there. Provisional until a ride trace shows daylight.
static NORMAL: Curve = Curve([(40, 10), (70, 25), (300, 55), (1_500, 85), (4_000, 100)]);
/// Lower than `NORMAL` at every light, and never above 70%.
static SAVER: Curve = Curve([(40, 5), (70, 15), (300, 35), (1_500, 55), (4_000, 70)]);

static TABLE: [Policy; 2] = [
    Policy {
        name: "NORMAL",
        glides: true,
        numbers: Numbers::OnArrival,
        note: "SMOOTH",
        motion_frame_ms: 40,
        light_curve: &NORMAL,
    },
    Policy {
        name: "SAVER",
        glides: false,
        numbers: Numbers::EverySecond,
        note: "NO GLIDES",
        motion_frame_ms: 40,
        light_curve: &SAVER,
    },
];

/// A row of `TABLE`, as stored in the preference record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayMode(u8);

impl DisplayMode {
    pub fn policy(self) -> &'static Policy {
        &TABLE[usize::from(self.0)]
    }

    /// The next mode, wrapping.
    pub fn next(self) -> Self {
        Self((self.0 + 1) % TABLE.len() as u8)
    }

    pub fn from_byte(byte: u8) -> Option<Self> {
        (usize::from(byte) < TABLE.len()).then_some(Self(byte))
    }

    pub const fn byte(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_rider_gets_the_first_row_and_next_walks_the_table_and_wraps() {
        let normal = DisplayMode::default();
        assert_eq!(normal.policy().name, "NORMAL");
        let saver = normal.next();
        assert_eq!(saver.policy().name, "SAVER");
        assert_eq!(saver.next(), normal);
    }

    #[test]
    fn only_rows_of_the_table_decode() {
        for byte in 0..TABLE.len() as u8 {
            assert_eq!(
                DisplayMode::from_byte(byte).map(DisplayMode::byte),
                Some(byte)
            );
        }
        assert_eq!(DisplayMode::from_byte(TABLE.len() as u8), None);
        assert_eq!(DisplayMode::from_byte(255), None);
    }

    #[test]
    fn saver_is_never_brighter_than_normal() {
        let [normal, saver] = &TABLE;
        for light in (0..=5_000).step_by(10) {
            assert!(
                saver.light_curve.percent(light) < normal.light_curve.percent(light),
                "{light}"
            );
        }
        assert_eq!(normal.light_curve.percent(u16::MAX), 100);
    }

    #[test]
    fn saver_trades_glides_for_numbers_once_a_second() {
        let [normal, saver] = &TABLE;
        assert!(normal.glides && normal.numbers == Numbers::OnArrival);
        assert!(!saver.glides && saver.numbers == Numbers::EverySecond);
    }
}
