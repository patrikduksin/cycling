//! The per-pixel renderer `View::paint` replaced, kept as the reference its
//! rows must match exactly. Delete it when a layout changes on purpose.
use super::*;
use firmware_shell::rendering::text::text;

impl View {
    pub(crate) fn reference_pixel(&self, x: usize, y: usize) -> u16 {
        const CYAN: u16 = 0x07ff;
        const WHITE: u16 = 0xffff;
        const DIM: u16 = 0x8410;
        if self.page == Page::Boot {
            let frame = self.now_ms / 180;
            let glitch = frame % 9 == 1 || frame % 9 == 2;
            let shift = if glitch && (y / 9 + frame as usize).is_multiple_of(3) {
                8
            } else {
                0
            };
            if text(x + shift, y, 24, 108, 8, b"VANA") {
                return WHITE;
            }
            if glitch
                && (text(x + shift + 4, y, 24, 106, 8, b"VANA")
                    || text(x + shift, y + 3, 28, 108, 8, b"VANA"))
            {
                return if y.is_multiple_of(2) { CYAN } else { 0xf81f };
            }
            if (y == 98 || y == 174) && (24..216).contains(&x) {
                return CYAN;
            }
            if text(x, y, 96, 195, 2, b"LABS") {
                return CYAN;
            }
            if text(x, y, 8, 269, 1, self.lines[0].as_bytes()) {
                return WHITE;
            }
            if y == 300 && x < ((self.now_ms % 6500) * 240 / 6500) as usize {
                return CYAN;
            }
            return 0;
        }
        if y < 29 {
            if text(x, y, 128, 7, 2, self.time.as_bytes()) {
                return WHITE;
            }
            if text(x, y, 8, 7, 2, if self.gps_fix { b"GPS" } else { b"GPS?" }) {
                return if self.gps_fix { CYAN } else { DIM };
            }
            if self.page == Page::Ride && text(x, y, 72, 7, 2, &self.ride_page.indicator()) {
                return DIM;
            }
            return 0;
        }
        if y == 29 || y == 285 {
            return 0x39e7;
        }
        if self.page == Page::Ride {
            return ride::reference_pixel(self, x, y);
        }
        if self.page == Page::Settings {
            return crate::screens::settings::reference_pixel(self, x, y);
        }
        if self.page == Page::Home {
            if text(x, y, 24, 50, 2, b"READY TO RIDE") {
                return DIM;
            }
            for (i, label) in HOME.iter().enumerate() {
                let top = 82 + i * 68;
                if (12..228).contains(&x) && (top..top + 58).contains(&y) {
                    let selected = self.cursor == i;
                    if text(x, y, 22, top + 18, 3, label) {
                        return if selected { 0 } else { WHITE };
                    }
                    if selected {
                        return CYAN;
                    }
                    if x == 12 || x == 227 || y == top || y == top + 57 {
                        return DIM;
                    }
                }
            }
        } else {
            for (i, line) in self.lines.iter().enumerate() {
                if text(
                    x,
                    y,
                    10,
                    36 + i * 27,
                    if i == 0
                        || (matches!(self.page, Page::Sensors | Page::Preflight)
                            && (2..=5).contains(&i))
                    {
                        1
                    } else {
                        2
                    },
                    line.as_bytes(),
                ) {
                    return if i == 0 { CYAN } else { WHITE };
                }
            }
        }
        let (left, right): (&[u8], &[u8]) = match self.page {
            Page::Home => (b"NEXT", b"SELECT"),
            Page::Sensors => (b"BACK", b"SCAN"),
            _ => (b"", b""),
        };
        if text(x, y, 8, 300, 2, left) || text(x, y, 156, 300, 2, right) {
            return CYAN;
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::radar::{Side, Target, Threat};
    use std::format;

    fn assert_matches(view: &View, case: &str) {
        for y in 0..320 {
            let mut pixels = [0u16; 240];
            view.paint(&mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, view.reference_pixel(x, y), "{case} at {x},{y}");
            }
        }
    }

    fn target(range_mm: u32, threat: Threat) -> Option<Target> {
        Some(Target {
            threat,
            side: Side::Behind,
            range_mm,
            closing_speed_cm_s: 900,
        })
    }

    #[test]
    fn rows_match_the_per_pixel_reference_on_every_page() {
        let pages = [
            Page::Boot,
            Page::Home,
            Page::Sensors,
            Page::Preflight,
            Page::Ride,
            Page::Settings,
        ];
        let long = "> RADIO READY / GPS SEARCHING FOR A FIX";
        for (p, page) in pages.into_iter().enumerate() {
            for cursor in 0..4 {
                for (editing, gps_fix, utc) in [(false, false, None), (true, true, Some(51_000))] {
                    for now_ms in [0, 180, 360, 1_000, 6_499, 7_777] {
                        let mut view = View::new(page, cursor, utc);
                        view.now_ms = now_ms;
                        view.gps_fix = gps_fix;
                        view.editing = editing;
                        for (i, line) in view.lines.iter_mut().enumerate() {
                            let _ = line.push_str(&long[..(9 + i * 4).min(long.len())]);
                        }
                        if editing {
                            view.lines[5] = Text::new();
                            let _ = view.lines[5].push_str("NOT SAVED: STORAGE");
                        }
                        view.prepare();
                        assert_matches(
                            &view,
                            &format!("page {p} cursor {cursor} editing {editing} at {now_ms}"),
                        );
                    }
                }
            }
        }
    }

    /// Laps of `seconds` each at alternating effort, so deltas take both signs.
    fn laps(count: u64, seconds: u64, sample: &Sample) -> Laps {
        let mut laps = Laps::default();
        let mut now = 0;
        for lap in 0..count {
            let mut effort = *sample;
            if lap % 2 == 1 {
                effort.power_watts = effort.power_watts.map(|w| w / 2);
                effort.speed_mm_s = effort.speed_mm_s.map(|v| v / 2);
            }
            for _ in 0..seconds {
                now += 1000;
                laps.add(
                    1000,
                    u64::from(effort.speed_mm_s.unwrap_or(0)),
                    &effort,
                    now,
                );
            }
            if lap + 1 < count {
                let _ = laps.lap(now);
            }
        }
        laps
    }

    /// Climbs after riding `metres` in 5 m steps along `altitude_dm(m)`.
    fn ridden(metres: u32, altitude_dm: impl Fn(u32) -> i32) -> Climbs {
        let mut climbs = Climbs::default();
        for m in (5..=metres).step_by(5) {
            climbs.add(1000, 5000, Some(altitude_dm(m)));
        }
        climbs
    }

    #[test]
    fn ride_rows_match_the_per_pixel_reference_in_every_state() {
        let samples = [
            Sample::default(),
            Sample {
                speed_mm_s: Some(8333),
                power_watts: Some(250),
                heart_bpm: Some(171),
                cadence_tenths: Some(904),
                gradient_tenths: Some(-35),
                ..Sample::default()
            },
            Sample {
                speed_mm_s: Some(5000),
                power_watts: Some(120),
                heart_bpm: Some(120),
                cadence_tenths: Some(800),
                gradient_tenths: Some(105),
                ..Sample::default()
            },
            Sample {
                speed_mm_s: Some(u32::MAX / 40),
                power_watts: Some(u16::MAX),
                heart_bpm: Some(u16::MAX),
                // The cadence rounding overflows at u16::MAX tenths.
                cadence_tenths: Some(65_000),
                gradient_tenths: Some(i16::MIN),
                ..Sample::default()
            },
        ];
        let radars = [
            radar::Snapshot::default(),
            radar::Snapshot {
                status: radar::Status::Ready,
                targets: [
                    target(0, Threat::FastApproach),
                    None,
                    target(4_000, Threat::Approach),
                    target(60_000, Threat::Approach),
                    target(61_000, Threat::FastApproach),
                    target(199_000, Threat::Approach),
                    target(500_000, Threat::FastApproach),
                    None,
                ],
            },
            radar::Snapshot {
                status: radar::Status::Error,
                ..radar::Snapshot::default()
            },
        ];
        let states = [
            Status::Ready,
            Status::Recording,
            Status::Paused,
            Status::Saved,
            Status::Recovered,
            Status::Error,
            Status::Full,
        ];
        let climbs = [
            Climbs::default(),
            // 1 km flat, then 1.5 km at 8%: on a climb.
            ridden(2500, |m| (m.saturating_sub(1000) * 8 / 10) as i32),
            // 1 km flat, 1 km at 7%, 1 km at 10%, then 400 m down: after one.
            ridden(3400, |m| match m {
                ..1000 => 0,
                1000..2000 => (m as i32 - 1000) * 7 / 10,
                2000..3000 => 700 + (m as i32 - 2000),
                _ => 1700 - (m as i32 - 3000) / 2,
            }),
        ];
        for (s, sample) in samples.into_iter().enumerate() {
            let history = [
                laps(1, 0, &sample),
                laps(3, 400, &sample),
                laps(2, 3_700, &sample),
            ];
            for (r, radar) in radars.into_iter().enumerate() {
                for (l, history) in history.iter().enumerate() {
                    for (t, state) in states.into_iter().enumerate() {
                        for stop_confirm in [false, true] {
                            for page in RidePage::ALL {
                                let mut view = View::new(Page::Ride, 0, Some(3_600));
                                view.sample = sample;
                                view.radar = radar;
                                view.state = state;
                                view.stop_confirm = stop_confirm;
                                view.storage_left_s = [None, Some(42 * 60), Some(4 * 3600)][t % 3];
                                view.active_secs = 3725 * s as u64;
                                view.ride_page = page;
                                if s == 2 {
                                    view.profile = TrainingProfile::new(300, 185).unwrap();
                                }
                                for _ in 0..l * 40 {
                                    let pressure = (l == 2).then_some(101_325 - s as u32 * 40);
                                    view.totals.add(1000, 8333 * s as u64, &sample, pressure);
                                    view.zone_time.add(1000, &sample, &view.profile);
                                }
                                view.climbs = climbs[(s + l) % 3].clone();
                                view.lap = history.current();
                                view.previous_lap = history.previous();
                                view.lap_toast = history.previous().filter(|_| s != 1);
                                view.prepare();
                                assert_matches(
                                    &view,
                                    &format!(
                                        "sample {s} radar {r} laps {l} {state:?} {stop_confirm} {page:?}"
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
