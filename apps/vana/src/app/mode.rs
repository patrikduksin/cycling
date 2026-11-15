use firmware_shell::backlight::Curve;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Numbers {
    OnArrival,
    EverySecond,
}

#[derive(Debug)]
pub struct Policy {
    pub name: &'static str,
    pub glides: bool,
    pub numbers: Numbers,
    pub note: &'static str,
    pub min_motion_frame_gap_ms: u64,
    pub light_curve: &'static Curve,
}

/// Readings are the companion's raw units: about 36 to 41 in a dark room at
/// night, 65 covered, 68 in a dim room, 470 to 1,557 under a phone torch.
/// No daylight reading is known yet, so the 4,000 knot is a guess until a
/// ride trace records one.
static NORMAL: Curve = Curve([(40, 10), (70, 25), (300, 55), (1_500, 85), (4_000, 100)]);
static SAVER: Curve = Curve([(40, 5), (70, 15), (300, 35), (1_500, 55), (4_000, 70)]);

static TABLE: [Policy; 2] = [
    Policy {
        name: "NORMAL",
        glides: true,
        numbers: Numbers::OnArrival,
        note: "SMOOTH",
        min_motion_frame_gap_ms: 40,
        light_curve: &NORMAL,
    },
    Policy {
        name: "SAVER",
        glides: false,
        numbers: Numbers::EverySecond,
        note: "NO GLIDES",
        min_motion_frame_gap_ms: 40,
        light_curve: &SAVER,
    },
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayMode(u8);

impl DisplayMode {
    pub fn policy(self) -> &'static Policy {
        &TABLE[usize::from(self.0)]
    }

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
    fn every_curve_in_the_table_rises_so_the_backlight_cannot_underflow() {
        for policy in &TABLE {
            assert!(policy.light_curve.rises(), "{}", policy.name);
        }
    }

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
