//! Logical 240x320 RGB565 status screen for the outdoor ANT sensor capture test.
//! The caller supplies live connection and verified storage-commit state.

pub(crate) use crate::ui_text::{number, text};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SensorState {
    /// No sensor selected.
    #[default]
    Off,
    /// Selected, but no fresh packets.
    Wait,
    /// Selected sensor packets are fresh.
    On,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CaptureState {
    #[default]
    Idle,
    Preparing,
    Recording,
    Stopping,
    Stopped,
    Error,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Screen {
    pub state: CaptureState,
    pub remaining_secs: Option<u32>,
    pub free_slots: Option<u32>,
    pub saved_environment: u32,
    pub power_watts: Option<u16>,
    pub radar_age_secs: Option<u32>,
    pub power_age_secs: Option<u32>,
    pub sampled: bool,
    /// Independent packet freshness in RADAR, HEART, POWER order.
    pub sensors: [SensorState; 3],
    /// True only after the capture has verified a storage commit.
    pub logging: bool,
    pub saved_packets: u32,
    /// Fresh live position fix, independent of capture commits.
    pub gps_fix: bool,
    /// Verified GPS records, including records without a fix.
    pub saved_positions: u32,
    pub dropped: u32,
    pub elapsed_secs: u32,
    pub error: bool,
}

const BLACK: u16 = 0x0000;
const WHITE: u16 = 0xffff;
const GREEN: u16 = 0x07e0;
const RED: u16 = 0xf800;
const YELLOW: u16 = 0xffe0;

/// No allocation, text formatting or retained framebuffer is required.
pub fn pixel(x: usize, y: usize, state: Screen) -> u16 {
    if x >= 240 || y >= 320 {
        return BLACK;
    }
    if text(x, y, 39, 8, 3, b"RIDE TEST") {
        return WHITE;
    }
    let (label, color) = match if state.error {
        CaptureState::Error
    } else {
        state.state
    } {
        CaptureState::Idle => (b"IDLE".as_slice(), WHITE),
        CaptureState::Preparing => (b"PREPARING".as_slice(), YELLOW),
        CaptureState::Recording if !state.logging => (b"WAIT SAVE".as_slice(), YELLOW),
        CaptureState::Recording => (b"RECORDING".as_slice(), GREEN),
        CaptureState::Stopping => (b"STOPPING".as_slice(), YELLOW),
        CaptureState::Stopped => (b"STOPPED".as_slice(), WHITE),
        CaptureState::Error => (b"ERROR".as_slice(), RED),
    };
    if text(x, y, (240 - label.len() * 12) / 2, 36, 2, label) {
        return color;
    }
    if (y == 58 || y == 208) && (8..232).contains(&x) {
        return WHITE;
    }
    if text(x, y, 8, 68, 2, b"TIME LEFT")
        || optional_number(x, y, 144, 68, 2, state.remaining_secs, 5)
        || text(x, y, 208, 68, 2, b"S")
    {
        return WHITE;
    }
    if text(x, y, 8, 94, 2, b"GPS")
        || text(
            x,
            y,
            144,
            94,
            2,
            if state.gps_fix { b"FIX" } else { b"WAIT" },
        )
    {
        return if state.gps_fix { GREEN } else { YELLOW };
    }
    let (radar, radar_color) = sensor_label(state.sensors[0]);
    if text(x, y, 8, 122, 3, b"RADAR") || text(x, y, 144, 122, 3, radar) {
        return radar_color;
    }
    if text(x, y, 8, 149, 1, b"PACKET AGE")
        || optional_number(x, y, 80, 149, 1, state.radar_age_secs, 5)
        || text(x, y, 112, 149, 1, b"S")
    {
        return radar_color;
    }
    let (power, power_color) = sensor_label(state.sensors[2]);
    // A cached watt value must never look live after packets become stale.
    let watts = if state.sensors[2] == SensorState::On {
        state.power_watts.map(u32::from)
    } else {
        None
    };
    if text(x, y, 8, 167, 3, b"POWER")
        || optional_number(x, y, 120, 167, 3, watts, 5)
        || text(x, y, 216, 167, 3, b"W")
    {
        return power_color;
    }
    if text(x, y, 8, 195, 1, power)
        || text(x, y, 44, 195, 1, b"AGE")
        || optional_number(x, y, 80, 195, 1, state.power_age_secs, 5)
        || text(x, y, 112, 195, 1, b"S")
    {
        return power_color;
    }
    for (top, label, value) in [
        (218, b"ENV SAVED".as_slice(), Some(state.saved_environment)),
        (238, b"GPS SAVED".as_slice(), Some(state.saved_positions)),
        (258, b"SLOTS FREE".as_slice(), state.free_slots),
        (278, b"DROPS".as_slice(), Some(state.dropped)),
    ] {
        if text(x, y, 8, top, 2, label) || optional_number(x, y, 152, top, 2, value, 6) {
            return if top == 278 && state.dropped != 0 {
                YELLOW
            } else {
                WHITE
            };
        }
    }
    if state.sampled && text(x, y, 66, 302, 2, b"SAMPLE 2S") {
        return YELLOW;
    }
    BLACK
}

fn sensor_label(state: SensorState) -> (&'static [u8], u16) {
    match state {
        SensorState::Off => (b"OFF", WHITE),
        SensorState::Wait => (b"WAIT", YELLOW),
        SensorState::On => (b"ON", GREEN),
    }
}

fn optional_number(
    x: usize,
    y: usize,
    left: usize,
    top: usize,
    scale: usize,
    value: Option<u32>,
    digits: usize,
) -> bool {
    match value {
        Some(value) => number(x, y, left, top, scale, value, digits),
        None => text(x, y, left, top, scale, b"--"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_states_have_distinct_labels_and_expected_colors() {
        let states = [
            (CaptureState::Idle, WHITE),
            (CaptureState::Preparing, YELLOW),
            (CaptureState::Recording, GREEN),
            (CaptureState::Stopping, YELLOW),
            (CaptureState::Stopped, WHITE),
            (CaptureState::Error, RED),
        ];
        let mut masks = [[false; 240 * 14]; 6];
        for (index, (capture, expected)) in states.into_iter().enumerate() {
            let state = Screen {
                state: capture,
                logging: true,
                ..Screen::default()
            };
            let mut mask = [false; 240 * 14];
            for y in 36..50 {
                for x in 0..240 {
                    let color = pixel(x, y, state);
                    assert!(color == BLACK || color == expected);
                    mask[(y - 36) * 240 + x] = color != BLACK;
                }
            }
            assert!(mask.iter().any(|&lit| lit));
            assert!(!masks[..index].contains(&mask));
            masks[index] = mask;
        }
    }

    #[test]
    fn recording_waits_for_verified_save_and_error_overrides_it() {
        for (logging, error, label, expected) in [
            (false, false, b"WAIT SAVE".as_slice(), YELLOW),
            (true, false, b"RECORDING".as_slice(), GREEN),
            (false, true, b"ERROR".as_slice(), RED),
            (true, true, b"ERROR".as_slice(), RED),
        ] {
            let state = Screen {
                state: CaptureState::Recording,
                logging,
                error,
                ..Screen::default()
            };
            let mut lit = 0;
            for y in 36..50 {
                for x in 0..240 {
                    let expected_pixel = if text(x, y, (240 - label.len() * 12) / 2, 36, 2, label) {
                        lit += 1;
                        expected
                    } else {
                        BLACK
                    };
                    assert_eq!(pixel(x, y, state), expected_pixel);
                }
            }
            assert!(lit > 100);
        }
    }

    #[test]
    fn missing_or_stale_power_uses_placeholder_not_cached_watts() {
        let missing = Screen::default();
        let stale = Screen {
            power_watts: Some(250),
            ..missing
        };
        for y in 167..188 {
            for x in 120..210 {
                assert_eq!(pixel(x, y, missing), pixel(x, y, stale));
                assert_eq!(
                    pixel(x, y, missing) != BLACK,
                    text(x, y, 120, 167, 3, b"--")
                );
            }
        }
    }

    #[test]
    fn numeric_fields_keep_zero_and_saturated_maximum_inside_panel() {
        for (left, scale, digits) in [(144, 2, 5), (120, 3, 5), (152, 2, 6)] {
            assert!(left + digits * 6 * scale <= 240);
            for value in [0, 600, u32::MAX] {
                let mut lit = 0;
                for y in 0..14 * scale {
                    for x in 0..260 {
                        let visible = number(x, y, left, 0, scale, value, digits);
                        if visible {
                            assert!(x < 240);
                            lit += 1;
                        }
                        if value == u32::MAX {
                            assert_eq!(
                                visible,
                                number(x, y, left, 0, scale, 10_u32.pow(digits as u32) - 1, digits)
                            );
                        }
                    }
                }
                assert!(lit > 0);
            }
        }
        assert_eq!(pixel(usize::MAX, usize::MAX, Screen::default()), BLACK);
    }
}
