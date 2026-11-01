//! ANT+ pages in words: what one packet said, by the sensor's device type.
//! Speed and cadence sensors send revolution counters, so a rate needs the
//! sensor's packet before; the reader keeps one per sensor.
//!
//! Field positions follow VANA's decoders in `apps/vana/src/sensors/ant.rs`
//! and `radar.rs`.
use device_api::ant::Identity;
use std::collections::HashMap;

const POWER: u8 = 11;
const RADAR: u8 = 40;
const HEART_RATE: u8 = 120;
const SPEED_CADENCE: u8 = 121;
const CADENCE: u8 = 122;
const SPEED: u8 = 123;

const BATTERY_PAGE: u8 = 0x52;
const HEART_RATE_BATTERY_PAGE: u8 = 0x07;

/// Each sensor's last packet, for the revolution rates.
#[derive(Default)]
pub struct Pages {
    last: HashMap<(u8, u16), [u8; 8]>,
}

impl Pages {
    pub fn describe(&mut self, identity: Identity, data: [u8; 8]) -> String {
        let before = self
            .last
            .insert((identity.device_type, identity.device_number), data);
        match identity.device_type {
            POWER => power(data),
            HEART_RATE => heart_rate(data),
            SPEED_CADENCE => format!(
                "{}, {}",
                revolutions("cadence", data, 0, before, Rate::Rpm),
                revolutions("wheel", data, 4, before, Rate::PerSecond)
            ),
            CADENCE => revolutions("cadence", data, 4, before, Rate::Rpm),
            SPEED => revolutions("wheel", data, 4, before, Rate::PerSecond),
            RADAR => radar(data),
            _ => page(data[0]),
        }
    }
}

fn page(number: u8) -> String {
    format!("page {number:#04x}")
}

fn common(data: [u8; 8]) -> String {
    match data[0] {
        BATTERY_PAGE => battery(data[6], data[7]),
        0x50 => "manufacturer info".into(),
        0x51 => "product info".into(),
        other => page(other),
    }
}

fn power(data: [u8; 8]) -> String {
    match data[0] {
        0x10 => format!(
            "power {} W, cadence {} (event {})",
            u16::from_le_bytes([data[6], data[7]]),
            match data[3] {
                0xff => "--".into(),
                rpm => format!("{rpm} rpm"),
            },
            data[1]
        ),
        0x01 => "calibration".into(),
        0x11 => "wheel torque".into(),
        0x12 => "crank torque".into(),
        0x13 => "torque effectiveness".into(),
        _ => common(data),
    }
}

fn heart_rate(data: [u8; 8]) -> String {
    if data[0] & 0x7f == HEART_RATE_BATTERY_PAGE {
        return battery(data[2], data[3]);
    }
    if data[0] == BATTERY_PAGE {
        return common(data);
    }
    let beats = data[6];
    match data[7] {
        0 => format!("hr -- (beat {beats})"),
        bpm => format!("hr {bpm} bpm (beat {beats})"),
    }
}

fn battery(fraction: u8, coarse: u8) -> String {
    let status = match coarse >> 4 & 0b111 {
        1 => "NEW",
        2 => "GOOD",
        3 => "OK",
        4 => "LOW",
        5 => "CRITICAL",
        _ => "status invalid",
    };
    // 0xf is "invalid"; zero is a sensor that does not measure.
    let volts = coarse & 0x0f;
    if volts == 0x0f || (volts == 0 && fraction == 0) {
        return format!("battery {status}");
    }
    format!(
        "battery {status} {:.2} V",
        f64::from(volts) + f64::from(fraction) / 256.0
    )
}

#[derive(Clone, Copy)]
enum Rate {
    Rpm,
    PerSecond,
}

fn revolutions(
    name: &str,
    data: [u8; 8],
    at: usize,
    before: Option<[u8; 8]>,
    rate: Rate,
) -> String {
    let read = |data: [u8; 8]| {
        (
            u16::from_le_bytes([data[at], data[at + 1]]),
            u16::from_le_bytes([data[at + 2], data[at + 3]]),
        )
    };
    let (time, count) = read(data);
    let Some((then, counted)) = before.map(read) else {
        return format!("{name} rev {count}");
    };
    let revs = count.wrapping_sub(counted);
    let seconds = f64::from(time.wrapping_sub(then)) / 1024.0;
    if revs == 0 || seconds == 0.0 {
        return format!("{name} rev {count} (no new revolution)");
    }
    let per_second = f64::from(revs) / seconds;
    match rate {
        Rate::Rpm => format!("{name} {:.0} rpm (rev {count})", per_second * 60.0),
        Rate::PerSecond => format!("{name} {per_second:.2} rev/s (rev {count})"),
    }
}

fn radar(data: [u8; 8]) -> String {
    match data[0] {
        0x30 | 0x31 => {
            let half = if data[0] == 0x30 { "A" } else { "B" };
            let ranges = u32::from_le_bytes([data[3], data[4], data[5], 0]);
            let targets: Vec<String> = (0..4)
                .filter_map(|i| {
                    let threat = match (data[1] >> (2 * i)) & 3 {
                        0 => return None,
                        1 => "approaching",
                        2 => "fast",
                        _ => "reserved threat",
                    };
                    let side = match (data[2] >> (2 * i)) & 3 {
                        0 => "behind",
                        1 => "right",
                        2 => "left",
                        _ => "side reserved",
                    };
                    let metres = f64::from((ranges >> (6 * i)) & 63) * 3.125;
                    let closing = f64::from((data[6 + i / 2] >> (4 * (i % 2))) & 15) * 3.04;
                    Some(format!("{side} {metres:.0} m {threat} {closing:.1} m/s"))
                })
                .collect();
            match targets.is_empty() {
                true => format!("radar targets {half}: none"),
                false => format!("radar targets {half}: {}", targets.join("; ")),
            }
        }
        0x57 => "radar error".into(),
        0x01 => "radar status".into(),
        _ => common(data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(device_type: u8) -> Identity {
        Identity {
            device_type,
            device_number: 1,
            transmission_type: 1,
        }
    }

    #[test]
    fn power_reads_watts_and_cadence_from_page_0x10() {
        let mut pages = Pages::default();
        assert_eq!(
            pages.describe(
                sensor(POWER),
                [0x10, 0xd3, 0xff, 88, 0x50, 0x49, 0xe6, 0x00]
            ),
            "power 230 W, cadence 88 rpm (event 211)"
        );
        assert_eq!(
            pages.describe(sensor(POWER), [0x10, 0xd4, 0xff, 0xff, 0, 0, 0, 0]),
            "power 0 W, cadence -- (event 212)"
        );
        assert_eq!(
            pages.describe(sensor(POWER), [0x52, 0xff, 0, 0, 0, 0, 0xfa, 0x22]),
            "battery GOOD 2.98 V"
        );
    }

    #[test]
    fn heart_rate_ignores_the_toggle_bit() {
        let mut pages = Pages::default();
        assert_eq!(
            pages.describe(sensor(HEART_RATE), [0x84, 0, 0, 0, 0x34, 0x12, 37, 142]),
            "hr 142 bpm (beat 37)"
        );
        assert_eq!(
            pages.describe(sensor(HEART_RATE), [0x04, 0, 0, 0, 0, 0, 38, 0]),
            "hr -- (beat 38)"
        );
    }

    #[test]
    fn speed_and_cadence_rates_come_from_the_packet_before() {
        let mut pages = Pages::default();
        let sensor = sensor(SPEED_CADENCE);
        assert_eq!(
            pages.describe(sensor, [0, 0, 10, 0, 0, 0, 100, 0]),
            "cadence rev 10, wheel rev 100"
        );
        assert_eq!(
            pages.describe(sensor, [0xcd, 0x02, 11, 0, 0x00, 0x04, 104, 0]),
            "cadence 86 rpm (rev 11), wheel 4.00 rev/s (rev 104)"
        );
        assert_eq!(
            pages.describe(sensor, [0xcd, 0x02, 11, 0, 0x00, 0x04, 104, 0]),
            "cadence rev 11 (no new revolution), wheel rev 104 (no new revolution)"
        );
    }

    #[test]
    fn radar_lists_each_target_with_side_range_and_closing_speed() {
        let mut pages = Pages::default();
        assert_eq!(
            pages.describe(sensor(RADAR), [0x30, 0b01, 0b00, 10, 0, 0, 3, 0]),
            "radar targets A: behind 31 m approaching 9.1 m/s"
        );
        assert_eq!(
            pages.describe(sensor(RADAR), [0x31, 0, 0, 0, 0, 0, 0, 0]),
            "radar targets B: none"
        );
    }
}
