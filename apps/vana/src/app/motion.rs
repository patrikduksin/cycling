//! Readings drawn between sensor samples. ANT+ sensors report about four
//! times a second, so the ride screen's numbers, zone gauges and radar
//! targets glide from one sample to the next instead of jumping. Only what
//! is drawn moves: recording, totals and radar alerts use the readings as
//! they arrive.
use crate::ride::log::Sample;
use crate::sensors::radar;
use firmware_shell::rendering::glide::Glide;

/// A number reaches each new reading in this long, a little under the
/// ~250 ms between ANT+ samples, so it settles before the next one.
const READING_MS: u32 = 200;
/// Radar targets glide quickly, so a marker never trails its vehicle.
const TARGET_MS: u32 = 120;
/// Between reports a target keeps closing at its reported speed, for at most
/// this long; a radar that goes quiet does not let a marker run on.
const EXTRAPOLATE_MS: u64 = 300;
/// A slot whose range jumps this far now holds another vehicle: it shows at
/// its new range at once rather than sliding there.
const NEW_VEHICLE_MM: i32 = 25_000;

#[derive(Default)]
pub(super) struct Motion {
    /// When the last frame was drawn.
    drawn_ms: u64,
    speed: Reading,
    power: Reading,
    heart: Reading,
    cadence: Reading,
    targets: [Track; 8],
}

struct Reading(Glide);

impl Default for Reading {
    fn default() -> Self {
        Self(Glide::new(READING_MS))
    }
}

impl Reading {
    fn show<T: Copy + Into<i64> + TryFrom<i64>>(&mut self, value: &mut Option<T>, now: u64) {
        let reading = value.and_then(|v| i32::try_from(v.into()).ok());
        self.0.set(reading, now);
        // A drawn value lies between two readings, so it fits their type.
        *value = self
            .0
            .at(now)
            .and_then(|drawn| T::try_from(i64::from(drawn)).ok())
            .or(*value);
    }
}

/// One radar slot: the report it last saw and where its marker is drawn.
struct Track {
    glide: Glide,
    seen: Option<radar::Target>,
    /// When the radar sent the report the marker moves on from.
    reported_ms: u64,
}

impl Default for Track {
    fn default() -> Self {
        Self {
            glide: Glide::new(TARGET_MS),
            seen: None,
            reported_ms: 0,
        }
    }
}

impl Track {
    /// How far a target closes from its report to `now`, in mm.
    fn closed(target: radar::Target, reported_ms: u64, now: u64) -> i32 {
        let closing_mm_s = i64::from(target.closing_speed_cm_s) * 10;
        let since = now.saturating_sub(reported_ms).min(EXTRAPOLATE_MS) as i64;
        (closing_mm_s * since / 1000) as i32
    }

    /// The drawn range at `now`: the glide, moved on by the closing speed
    /// since the last report.
    fn range(&self, now: u64) -> Option<i32> {
        let target = self.seen?;
        let closed = Self::closed(target, self.reported_ms, now);
        Some(self.glide.at(now)?.saturating_sub(closed).max(0))
    }

    /// A report the marker has not moved to yet: another target, or the
    /// same one reported again.
    fn is_new(&self, slot: Option<radar::Target>, received_ms: Option<u64>) -> bool {
        slot != self.seen || slot.is_some() && received_ms != Some(self.reported_ms)
    }

    fn show(&mut self, slot: &mut Option<radar::Target>, received_ms: Option<u64>, now: u64) {
        let Some(target) = *slot else {
            self.seen = None;
            self.glide.set(None, now);
            return;
        };
        if self.is_new(*slot, received_ms) {
            let reported = target.range_mm as i32;
            let reported_ms = received_ms.unwrap_or(now).min(now);
            // The marker stays where it is drawn and moves on from the new
            // report: the glide starts that much further out.
            let lead = Self::closed(target, reported_ms, now);
            match self.range(now) {
                Some(drawn) if (drawn - reported).abs() < NEW_VEHICLE_MM => {
                    self.glide.restart(drawn + lead, reported, now)
                }
                _ => {
                    self.glide.set(None, now);
                    self.glide.set(Some(reported), now);
                }
            }
            self.seen = Some(target);
            self.reported_ms = reported_ms;
        }
        if let (Some(drawn), Some(slot)) = (self.range(now), slot.as_mut()) {
            // Only the marker moves: the warning stays the reported one, as
            // the alert hears it, so a glide never turns a marker red early
            // or keeps it calm late.
            let mut drawn = drawn.max(0) as u32;
            if slot.closing_speed_cm_s > 0 {
                let edge = 40_000u32.max(u32::from(slot.closing_speed_cm_s) * 60);
                drawn = if radar::close(target) {
                    drawn.min(edge)
                } else {
                    drawn.max(edge + 1)
                };
            }
            slot.range_mm = drawn;
        }
    }

    fn moving(&self, now: u64) -> bool {
        self.glide.moving(now)
            || self.seen.is_some_and(|target| {
                target.closing_speed_cm_s > 0
                    && now.saturating_sub(self.reported_ms) < EXTRAPOLATE_MS
            })
    }
}

/// Radar reports with the times their halves (slots 0..4 and 4..8) arrived.
#[derive(Clone, Copy)]
pub(super) struct Reports {
    pub(super) snapshot: radar::Snapshot,
    pub(super) received_ms: [Option<u64>; 2],
}

impl Reports {
    fn received(&self, slot: usize) -> Option<u64> {
        self.received_ms[slot / 4]
    }
}

/// What a ride frame shows of the gliding values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Visible {
    /// The main page's speed, power, heart rate and cadence.
    pub readings: bool,
    /// Radar markers.
    pub radar: bool,
}

impl Motion {
    /// Take the latest readings and replace them with what to draw at `now`.
    pub(super) fn show(&mut self, sample: &mut Sample, radar: &mut Reports, now: u64) {
        self.drawn_ms = now;
        self.speed.show(&mut sample.speed_mm_s, now);
        self.power.show(&mut sample.power_watts, now);
        self.heart.show(&mut sample.heart_bpm, now);
        self.cadence.show(&mut sample.cadence_tenths, now);
        let received = radar.received_ms;
        for (slot, (track, target)) in self
            .targets
            .iter_mut()
            .zip(radar.snapshot.targets.iter_mut())
            .enumerate()
        {
            track.show(target, received[slot / 4], now);
        }
    }

    /// Something shown is still on its way to its reading at `now`.
    pub(super) fn moving(&self, visible: Visible, now: u64) -> bool {
        (visible.readings
            && [&self.speed, &self.power, &self.heart, &self.cadence]
                .iter()
                .any(|reading| reading.0.moving(now)))
            || (visible.radar && self.targets.iter().any(|track| track.moving(now)))
    }

    /// When a sample or radar report not yet drawn should start its glide:
    /// one motion frame after the last drawn frame. None while nothing new
    /// arrived.
    pub(super) fn due(
        &self,
        visible: Visible,
        sample: &Sample,
        radar: &Reports,
        frame_ms: u64,
    ) -> Option<u64> {
        let reading =
            |glide: &Reading, value: Option<i64>| glide.0.reading().map(i64::from) != value;
        let new_reading = visible.readings
            && (reading(&self.speed, sample.speed_mm_s.map(i64::from))
                || reading(&self.power, sample.power_watts.map(i64::from))
                || reading(&self.heart, sample.heart_bpm.map(i64::from))
                || reading(&self.cadence, sample.cadence_tenths.map(i64::from)));
        let new_report = visible.radar
            && self.targets.iter().enumerate().any(|(slot, track)| {
                track.is_new(radar.snapshot.targets[slot], radar.received(slot))
            });
        (new_reading || new_report).then_some(self.drawn_ms + frame_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Visible = Visible {
        readings: true,
        radar: true,
    };

    fn sample(speed: u32, watts: u16) -> Sample {
        Sample {
            speed_mm_s: Some(speed),
            power_watts: Some(watts),
            heart_bpm: Some(140),
            cadence_tenths: None,
            ..Sample::default()
        }
    }

    fn quiet() -> Reports {
        Reports {
            snapshot: radar::Snapshot::default(),
            received_ms: [None; 2],
        }
    }

    #[test]
    fn numbers_glide_between_samples_and_settle_on_them() {
        let mut motion = Motion::default();
        let mut first = sample(8_000, 200);
        motion.show(&mut first, &mut quiet(), 0);
        assert_eq!(first, sample(8_000, 200), "the first sample shows at once");
        assert!(!motion.moving(ALL, 0));
        assert_eq!(motion.due(ALL, &sample(8_000, 200), &quiet(), 40), None);
        // A new sample asks for a frame one motion frame after the last.
        assert_eq!(motion.due(ALL, &sample(9_000, 300), &quiet(), 40), Some(40));
        let mut drawn = std::vec::Vec::new();
        for now in (250..=500).step_by(40) {
            let mut next = sample(9_000, 300);
            motion.show(&mut next, &mut quiet(), now);
            drawn.push((next.speed_mm_s.unwrap(), next.power_watts.unwrap()));
            assert_eq!(next.heart_bpm, Some(140));
            assert_eq!(next.cadence_tenths, None);
        }
        assert!(
            drawn
                .windows(2)
                .all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1)
        );
        assert!(drawn[1].0 > 8_000 && drawn[1].0 < 9_000, "{drawn:?}");
        assert_eq!(*drawn.last().unwrap(), (9_000, 300));
        assert!(motion.moving(ALL, 300));
        assert!(!motion.moving(ALL, 450));
        assert_eq!(motion.due(ALL, &sample(9_000, 300), &quiet(), 40), None);
    }

    #[test]
    fn a_lost_reading_shows_at_once() {
        let mut motion = Motion::default();
        motion.show(&mut sample(8_000, 200), &mut quiet(), 0);
        let mut lost = Sample::default();
        motion.show(&mut lost, &mut quiet(), 100);
        assert_eq!(lost.speed_mm_s, None);
        assert_eq!(lost.power_watts, None);
        assert!(!motion.moving(ALL, 100));
    }

    fn target(range_mm: u32, closing_speed_cm_s: u16) -> Option<radar::Target> {
        Some(radar::Target {
            threat: radar::Threat::Approach,
            side: radar::Side::Behind,
            range_mm,
            closing_speed_cm_s,
        })
    }

    fn report(range_mm: u32, speed: u16, received_ms: u64) -> Reports {
        let mut reports = quiet();
        reports.snapshot.targets[0] = target(range_mm, speed);
        reports.received_ms[0] = Some(received_ms);
        reports
    }

    fn range(reports: &Reports) -> u32 {
        reports.snapshot.targets[0].unwrap().range_mm
    }

    #[test]
    fn radar_targets_close_between_reports_and_glide_to_the_next() {
        let mut motion = Motion::default();
        let mut sample = Sample::default();
        // 100 m away, closing at 12.16 m/s (four of the profile's 3.04 m/s steps).
        let mut drawn = report(100_000, 1_216, 0);
        motion.show(&mut sample, &mut drawn, 0);
        assert_eq!(range(&drawn), 100_000);
        let mut last = 100_000;
        for now in (40..=240).step_by(40) {
            let mut drawn = report(100_000, 1_216, 0);
            assert_eq!(motion.due(ALL, &sample, &drawn, 40), None, "nothing new");
            motion.show(&mut sample, &mut drawn, now);
            assert!(range(&drawn) < last, "closes between reports");
            last = range(&drawn);
        }
        assert_eq!(last, 100_000 - 12_160 * 240 / 1000);
        // The next report, received at 245 ms and drawn at 250 ms, starts
        // from the marker rather than from its own range.
        let next = report(96_875, 1_216, 245);
        assert_eq!(motion.due(ALL, &sample, &next, 40), Some(280));
        let mut drawn = next;
        motion.show(&mut sample, &mut drawn, 250);
        assert!(
            range(&drawn).abs_diff(last) < 200,
            "{} vs {last}",
            range(&drawn)
        );
        // The same range reported again is still a fresh report: the marker
        // keeps closing rather than stopping 300 ms after the first.
        for (received, now) in [(495, 500), (745, 750)] {
            let mut drawn = report(96_875, 1_216, received);
            motion.show(&mut sample, &mut drawn, now);
            assert!(motion.moving(ALL, now + 100), "at {now}");
            assert!(range(&drawn) < 96_875);
        }
        // A quiet radar does not move the marker on.
        let mut drawn = report(96_875, 1_216, 745);
        motion.show(&mut sample, &mut drawn, 3_000);
        assert_eq!(range(&drawn), 96_875 - 12_160 * 300 / 1000);
        assert!(!motion.moving(ALL, 3_000));
        // Another vehicle in the slot shows at its own range at once.
        let mut drawn = report(190_000, 0, 3_000);
        motion.show(&mut sample, &mut drawn, 3_040);
        assert_eq!(range(&drawn), 190_000);
        // A cleared slot clears its marker.
        let mut drawn = quiet();
        motion.show(&mut sample, &mut drawn, 3_080);
        assert_eq!(drawn.snapshot.targets[0], None);
        assert!(!motion.moving(ALL, 3_080));
    }

    #[test]
    fn the_marker_never_goes_past_the_rider() {
        let mut motion = Motion::default();
        let mut sample = Sample::default();
        motion.show(&mut sample, &mut report(3_125, 4_560, 0), 0);
        let mut later = report(3_125, 4_560, 0);
        motion.show(&mut sample, &mut later, 290);
        assert_eq!(range(&later), 0);
    }

    #[test]
    fn hidden_values_never_ask_for_motion_frames() {
        let mut motion = Motion::default();
        let mut shown = sample(8_000, 200);
        motion.show(&mut shown, &mut quiet(), 0);
        let hidden = Visible::default();
        assert_eq!(motion.due(hidden, &sample(9_000, 300), &quiet(), 40), None);
        let mut next = sample(9_000, 300);
        motion.show(&mut next, &mut quiet(), 100);
        assert!(motion.moving(ALL, 150));
        assert!(!motion.moving(hidden, 150));
    }

    #[test]
    fn a_gliding_marker_keeps_its_reported_warning() {
        let target = |range_mm, closing_speed_cm_s| radar::Target {
            threat: radar::Threat::Approach,
            side: radar::Side::Behind,
            range_mm,
            closing_speed_cm_s,
        };
        let mut track = Track::default();
        // Not close: 40.625 m at 3.04 m/s, whose glide would cross 40 m.
        let far = target(40_625, 304);
        assert!(!radar::close(far));
        let mut slot = Some(far);
        track.show(&mut slot, Some(0), 0);
        let mut slot = Some(far);
        track.show(&mut slot, Some(0), 250);
        assert!(!radar::close(slot.unwrap()), "{slot:?}");
        // A newly close vehicle far from the drawn one is red at once.
        let near = target(39_000, 304);
        let mut slot = Some(near);
        track.show(&mut slot, Some(300), 300);
        assert!(radar::close(slot.unwrap()), "{slot:?}");
    }
}
