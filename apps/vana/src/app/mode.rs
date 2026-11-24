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
    pub min_motion_frame_gap_ms: u64,
}

static TABLE: [Policy; 2] = [
    Policy {
        name: "NORMAL",
        glides: true,
        numbers: Numbers::OnArrival,
        min_motion_frame_gap_ms: 40,
    },
    Policy {
        name: "SAVER",
        glides: false,
        numbers: Numbers::EverySecond,
        min_motion_frame_gap_ms: 40,
    },
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayMode(u8);

impl DisplayMode {
    pub fn policy(self) -> &'static Policy {
        &TABLE[usize::from(self.0)]
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
    fn a_new_rider_gets_the_first_row() {
        assert_eq!(DisplayMode::default().policy().name, "NORMAL");
        assert_eq!(DisplayMode::from_byte(1).unwrap().policy().name, "SAVER");
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
    fn saver_trades_glides_for_numbers_once_a_second() {
        let [normal, saver] = &TABLE;
        assert!(normal.glides && normal.numbers == Numbers::OnArrival);
        assert!(!saver.glides && saver.numbers == Numbers::EverySecond);
    }
}
