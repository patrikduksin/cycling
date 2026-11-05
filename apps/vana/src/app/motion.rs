//! Graphics drawn between sensor samples. ANT+ sensors report about four
//! times a second, so the main page's zone gauges and the radar markers
//! glide from one sample to the next instead of jumping. Numbers never
//! glide: a rider glancing down mid-glide could not read one, so they show
//! each reading as it arrives. Recording, totals and radar alerts use the
//! readings as they arrive too.
use crate::ride::log::Sample;
use crate::screens::ride::Pixels;
use crate::screens::workout::Graphics;
use crate::sensors::radar;
use firmware_shell::rendering::glide::Glide;

/// A gauge reaches each new reading in this long, a little under the
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
/// A glide's next pixel is looked for in steps this fine, so its frame
/// comes about when the pixel moves.
const STEP_MS: u64 = 10;

pub(super) struct Motion {
    /// When the last frame was drawn, and what it drew.
    drawn_ms: u64,
    numbers: Numbers,
    pixels: Pixels,
    power: Glide,
    heart: Glide,
    radar: radar::Status,
    targets: [Track; 8],
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            drawn_ms: 0,
            numbers: Numbers::default(),
            pixels: Pixels::default(),
            power: Glide::new(READING_MS),
            heart: Glide::new(READING_MS),
            radar: radar::Status::default(),
            targets: Default::default(),
        }
    }
}

/// The readings the main page writes as numbers.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Numbers {
    speed_mm_s: Option<u32>,
    power_watts: Option<u16>,
    heart_bpm: Option<u16>,
    cadence_tenths: Option<u16>,
}

impl From<&Sample> for Numbers {
    fn from(sample: &Sample) -> Self {
        Self {
            speed_mm_s: sample.speed_mm_s,
            power_watts: sample.power_watts,
            heart_bpm: sample.heart_bpm,
            cadence_tenths: sample.cadence_tenths,
        }
    }
}

/// One radar slot: the report it last saw and where its marker glides.
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

    fn take(&mut self, slot: Option<radar::Target>, received_ms: Option<u64>, now: u64) {
        let Some(target) = slot else {
            self.seen = None;
            self.glide.set(None, now);
            return;
        };
        if !self.is_new(slot, received_ms) {
            return;
        }
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

    /// The marker at `now`: the report, at its drawn range.
    fn drawn(&self, now: u64) -> Option<radar::Target> {
        let mut target = self.seen?;
        let mut drawn = self.range(now)?.max(0) as u32;
        // Only the marker moves: the warning stays the reported one, as the
        // alert hears it, so a glide never turns a marker red early or keeps
        // it calm late.
        if target.closing_speed_cm_s > 0 {
            let edge = 40_000u32.max(u32::from(target.closing_speed_cm_s) * 60);
            drawn = if radar::close(target) {
                drawn.min(edge)
            } else {
                drawn.max(edge + 1)
            };
        }
        target.range_mm = drawn;
        Some(target)
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

/// What a ride frame shows of the readings and gliding graphics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Visible {
    /// The main page: its numbers and zone gauges.
    pub main: bool,
    /// Radar markers.
    pub radar: bool,
}

impl Motion {
    /// Take the latest readings, `sample` with power as its 3 s average:
    /// a changed one starts its glide at `now`.
    pub(super) fn take(&mut self, sample: &Sample, radar: &Reports, now: u64) {
        self.power.set(sample.power_watts.map(i32::from), now);
        self.heart.set(sample.heart_bpm.map(i32::from), now);
        self.radar = radar.snapshot.status;
        for (slot, track) in self.targets.iter_mut().enumerate() {
            track.take(radar.snapshot.targets[slot], radar.received(slot), now);
        }
    }

    /// The graphics a frame at `now` draws, without those `visible` hides.
    pub(super) fn graphics(&self, visible: Visible, now: u64) -> Graphics {
        // A drawn value lies between two readings, so it fits their type.
        let at = |glide: &Glide| {
            glide
                .at(now)
                .and_then(|drawn| u16::try_from(drawn).ok())
                .filter(|_| visible.main)
        };
        let mut radar = radar::Snapshot {
            status: self.radar,
            ..radar::Snapshot::default()
        };
        if visible.radar {
            for (target, track) in radar.targets.iter_mut().zip(&self.targets) {
                *target = track.drawn(now);
            }
        }
        Graphics {
            power_3s: at(&self.power),
            heart_bpm: at(&self.heart),
            radar,
        }
    }

    /// The frame drawn at `now` showed `sample`'s numbers and its graphics
    /// at `pixels`.
    pub(super) fn drew(&mut self, sample: &Sample, pixels: Pixels, now: u64) {
        self.drawn_ms = now;
        self.numbers = sample.into();
        self.pixels = pixels;
    }

    /// When the next frame should draw what changed since the last one,
    /// at most one every `frame_ms`: a new number as it comes, a glide
    /// once a graphic it moves would land on another pixel. None while
    /// neither.
    pub(super) fn due(
        &self,
        visible: Visible,
        sample: &Sample,
        frame_ms: u64,
        now: u64,
        pixels: impl Fn(&Graphics) -> Pixels,
    ) -> Option<u64> {
        // A sheet over the page draws no frame `drew` hears of.
        if visible == Visible::default() {
            return None;
        }
        let mut at = now.max(self.drawn_ms + frame_ms);
        if visible.main && Numbers::from(sample) != self.numbers {
            return Some(at);
        }
        loop {
            if pixels(&self.graphics(visible, at)) != self.pixels {
                return Some(at);
            }
            if !self.moving(visible, at) {
                return None;
            }
            at += STEP_MS;
        }
    }

    /// Something shown is still on its way to its reading at `now`.
    fn moving(&self, visible: Visible, now: u64) -> bool {
        (visible.main && (self.power.moving(now) || self.heart.moving(now)))
            || (visible.radar && self.targets.iter().any(|track| track.moving(now)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::zones::TrainingProfile;
    use crate::screens::ride::pixels;

    const ALL: Visible = Visible {
        main: true,
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

    fn on_screen(graphics: &Graphics) -> Pixels {
        pixels(graphics, &TrainingProfile::default())
    }

    /// Take `sample` and `radar` and draw a frame at `now`, as the runtime
    /// does.
    fn draw(motion: &mut Motion, sample: &Sample, radar: &Reports, now: u64) -> Graphics {
        motion.take(sample, radar, now);
        let graphics = motion.graphics(ALL, now);
        motion.drew(sample, on_screen(&graphics), now);
        graphics
    }

    fn due(motion: &Motion, visible: Visible, sample: &Sample, now: u64) -> Option<u64> {
        motion.due(visible, sample, 40, now, on_screen)
    }

    #[test]
    fn a_new_number_asks_for_a_frame_one_motion_frame_after_the_last() {
        let mut motion = Motion::default();
        draw(&mut motion, &sample(8_000, 200), &quiet(), 0);
        assert_eq!(due(&motion, ALL, &sample(8_000, 200), 10), None);
        assert_eq!(due(&motion, ALL, &sample(9_000, 200), 10), Some(40));
        assert_eq!(
            due(&motion, ALL, &sample(9_000, 200), 100),
            Some(100),
            "at once when the last frame is a motion frame ago"
        );
    }

    #[test]
    fn gauges_glide_between_samples_and_settle_on_them() {
        let mut motion = Motion::default();
        let first = draw(&mut motion, &sample(8_000, 200), &quiet(), 0);
        assert_eq!(first.power_3s, Some(200), "the first sample shows at once");
        assert_eq!(first.heart_bpm, Some(140));
        motion.take(&sample(9_000, 300), &quiet(), 250);
        let drawn: std::vec::Vec<_> = (250..=500)
            .step_by(40)
            .map(|now| motion.graphics(ALL, now).power_3s.unwrap())
            .collect();
        assert!(drawn.windows(2).all(|w| w[0] <= w[1]), "{drawn:?}");
        assert!(drawn[1] > 200 && drawn[1] < 300, "{drawn:?}");
        assert_eq!(*drawn.last().unwrap(), 300);
        assert_eq!(motion.graphics(ALL, 450).heart_bpm, Some(140));
    }

    #[test]
    fn a_lost_reading_clears_its_gauge_at_once() {
        let mut motion = Motion::default();
        draw(&mut motion, &sample(8_000, 200), &quiet(), 0);
        let lost = draw(&mut motion, &Sample::default(), &quiet(), 100);
        assert_eq!((lost.power_3s, lost.heart_bpm), (None, None));
        assert_eq!(due(&motion, ALL, &Sample::default(), 100), None);
    }

    /// Every frame a glide asks for moves a pixel, the first time one moves,
    /// and a glide that moves none asks for no frame.
    #[test]
    fn a_glide_asks_for_a_frame_only_when_a_pixel_moves() {
        let (mut still, mut moved) = (0, 0);
        for to in 200..=260 {
            let mut motion = Motion::default();
            draw(&mut motion, &sample(8_000, 200), &quiet(), 0);
            let next = sample(8_000, to);
            draw(&mut motion, &next, &quiet(), 250);
            let before = on_screen(&motion.graphics(ALL, 250));
            let due = due(&motion, ALL, &next, 250);
            let first_move =
                (290..=450).find(|&now| on_screen(&motion.graphics(ALL, now)) != before);
            match (due, first_move) {
                (None, None) => still += 1,
                (Some(due), Some(first)) => {
                    assert!(
                        due >= first && due < first + STEP_MS,
                        "{to} W: {due} vs {first}"
                    );
                    moved += 1;
                }
                _ => panic!("{to} W: due {due:?}, first move {first_move:?}"),
            }
        }
        assert!(still > 0 && moved > 0, "{still} still, {moved} moved");
    }

    #[test]
    fn hidden_graphics_never_ask_for_frames() {
        let mut motion = Motion::default();
        draw(
            &mut motion,
            &sample(8_000, 200),
            &report(100_000, 1_216, 0),
            0,
        );
        let next = sample(8_000, 300);
        draw(&mut motion, &next, &report(100_000, 1_216, 0), 100);
        assert!(due(&motion, ALL, &next, 100).is_some());
        let hidden = Visible::default();
        let shown = motion.graphics(hidden, 150);
        assert_eq!((shown.power_3s, shown.radar.targets[0]), (None, None));
        motion.drew(&next, on_screen(&shown), 150);
        assert_eq!(due(&motion, hidden, &next, 150), None);
        assert_eq!(
            due(&motion, hidden, &sample(9_000, 310), 150),
            None,
            "nor numbers"
        );
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

    fn range(graphics: &Graphics) -> u32 {
        graphics.radar.targets[0].unwrap().range_mm
    }

    #[test]
    fn radar_targets_close_between_reports_and_glide_to_the_next() {
        let mut motion = Motion::default();
        let sample = Sample::default();
        // 100 m away, closing at 12.16 m/s (four of the profile's 3.04 m/s steps).
        let drawn = draw(&mut motion, &sample, &report(100_000, 1_216, 0), 0);
        assert_eq!(range(&drawn), 100_000);
        let mut last = 100_000;
        for now in (40..=240).step_by(40) {
            let drawn = draw(&mut motion, &sample, &report(100_000, 1_216, 0), now);
            assert!(range(&drawn) < last, "closes between reports");
            last = range(&drawn);
        }
        assert_eq!(last, 100_000 - 12_160 * 240 / 1000);
        // The next report, received at 245 ms and taken at 250 ms, starts
        // from the marker rather than from its own range.
        let next = report(96_875, 1_216, 245);
        motion.take(&sample, &next, 250);
        let drawn = motion.graphics(ALL, 250);
        assert!(
            range(&drawn).abs_diff(last) < 200,
            "{} vs {last}",
            range(&drawn)
        );
        // The same range reported again is still a fresh report: the marker
        // keeps closing rather than stopping 300 ms after the first.
        for (received, now) in [(495, 500), (745, 750)] {
            let drawn = draw(&mut motion, &sample, &report(96_875, 1_216, received), now);
            assert!(range(&drawn) < 96_875);
            assert!(
                due(&motion, ALL, &sample, now).is_some(),
                "still closing at {now}"
            );
        }
        // A quiet radar does not move the marker on.
        let drawn = draw(&mut motion, &sample, &report(96_875, 1_216, 745), 3_000);
        assert_eq!(range(&drawn), 96_875 - 12_160 * 300 / 1000);
        assert_eq!(due(&motion, ALL, &sample, 3_000), None);
        // Another vehicle in the slot shows at its own range at once.
        let drawn = draw(&mut motion, &sample, &report(190_000, 0, 3_000), 3_040);
        assert_eq!(range(&drawn), 190_000);
        // A cleared slot clears its marker, on the next frame.
        motion.take(&sample, &quiet(), 3_080);
        assert_eq!(due(&motion, ALL, &sample, 3_090), Some(3_090));
        assert_eq!(motion.graphics(ALL, 3_090).radar.targets[0], None);
    }

    #[test]
    fn the_marker_never_goes_past_the_rider() {
        let mut motion = Motion::default();
        let sample = Sample::default();
        draw(&mut motion, &sample, &report(3_125, 4_560, 0), 0);
        let later = draw(&mut motion, &sample, &report(3_125, 4_560, 0), 290);
        assert_eq!(range(&later), 0);
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
        track.take(Some(far), Some(0), 0);
        let drawn = track.drawn(250).unwrap();
        assert!(!radar::close(drawn), "{drawn:?}");
        // A newly close vehicle far from the drawn one is red at once.
        let near = target(39_000, 304);
        track.take(Some(near), Some(300), 300);
        let drawn = track.drawn(300).unwrap();
        assert!(radar::close(drawn), "{drawn:?}");
    }
}
