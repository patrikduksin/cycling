//! Playing a workout during a ride: which step is on, how much of it is
//! left, whether the rider is on target, and what FTP a test measured.
//!
//! The player advances only with recorded time and distance, the same steps
//! the ride totals take, so a pause stops the workout with the ride. Power is
//! checked as a 3 s average (`Power3s`, local to the player: the ride has no
//! shared smoothed power yet); heart rate and cadence as read. A lap press
//! ends the step (`skip`), and each step that ends closes a lap.
use super::model::{Band, Intensity, Length, MAX_PLAYED, Metric, Played, Target, Test, Workout};
use crate::ride::log::Sample;
use crate::ride::zones::TrainingProfile;

/// The rider counts down the last seconds of a timed step.
pub const COUNTDOWN_S: u32 = 3;
/// A ramp ends after the rider stays under this share of its watts...
const RAMP_FAIL_PCT: u32 = 90;
/// ...for this long.
const RAMP_FAIL_MS: u64 = 10_000;
/// Seconds in the best-minute window of a ramp.
const MINUTE: usize = 60;
/// What a touch adds to the step on screen.
pub const EXTEND_S: u32 = 60;
pub const EXTEND_M: u32 = 500;

/// Where the reading sits against the step's band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compliance {
    Under,
    In,
    Over,
}

/// What the player tells the ride after a step of riding or a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// `n` seconds are left of a timed step.
    Countdown(u32),
    /// Played step `position` began; the one before ended.
    Started(usize),
    /// The last step ended.
    Finished,
    /// A test measured this FTP, which the rider may save.
    Ftp(u16),
}

/// Room for every event one odometer update (at most a few seconds and
/// metres over `MIN_DISTANCE_M`) can raise.
pub type Events = heapless::Vec<Event, 16>;

/// Average power over the last three seconds of riding.
#[derive(Clone, Copy, Debug, Default)]
pub struct Power3s {
    watts: [Option<u16>; 3],
    next: usize,
}

impl Power3s {
    pub fn add(&mut self, watts: Option<u16>) {
        self.watts[self.next] = watts;
        self.next = (self.next + 1) % self.watts.len();
    }

    /// The mean of the readings in the window, `None` without any.
    pub fn watts(&self) -> Option<u16> {
        let readings = self.watts.iter().flatten();
        let count = readings.clone().count() as u32;
        (count > 0).then(|| {
            let sum: u32 = readings.map(|&w| u32::from(w)).sum();
            ((sum + count / 2) / count) as u16
        })
    }
}

/// How the whole workout went so far, for the ride summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Played steps finished, skipped ones included.
    pub steps_done: u16,
    pub steps: u16,
    pub skipped: u16,
    /// Recorded time with a target and a reading, and its split.
    pub targeted_ms: u64,
    pub in_ms: u64,
    pub under_ms: u64,
    pub over_ms: u64,
    pub finished: bool,
    /// What a test measured, saved or not.
    pub ftp: Option<u16>,
}

impl Summary {
    /// Share of targeted time on target, in whole percent.
    pub fn in_target_pct(&self) -> Option<u8> {
        (self.targeted_ms > 0)
            .then(|| ((self.in_ms * 100 + self.targeted_ms / 2) / self.targeted_ms) as u8)
    }
}

/// What a screen needs about the step on now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub position: usize,
    pub steps: usize,
    pub played: Played,
    pub band: Option<Band>,
    /// The reading the band is checked against.
    pub reading: Option<u16>,
    pub compliance: Option<Compliance>,
    pub left: Left,
    /// How far through the step, in thousandths; zero for open steps.
    pub progress: u16,
    pub next: Option<Played>,
    pub test: Option<TestReadout>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    Secs(u32),
    Metres(u32),
    /// Open step: how long it has run.
    Open(u32),
}

/// A test's running number on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestReadout {
    /// Average watts of the twenty minutes so far.
    Average(u16),
    /// Best minute of the ramp so far.
    BestMinute(u16),
}

pub struct Player {
    workout: Workout,
    steps: usize,
    position: usize,
    played: Played,
    step_ms: u64,
    step_mm: u64,
    extra_s: u32,
    extra_m: u32,
    power: Power3s,
    last_sample: Sample,
    summary: Summary,
    /// The twenty minutes' energy and time.
    test_mj: u64,
    test_ms: u64,
    /// The ramp's last minute of watts, its sum and the best full minute.
    minute: [u16; MINUTE],
    minute_len: usize,
    minute_at: usize,
    minute_rest_ms: u64,
    best_minute: Option<u16>,
    ramp_low_ms: u64,
    done: bool,
}

impl Player {
    pub fn new(workout: Workout) -> Self {
        let steps = workout.played_len();
        let played = workout.played().next().unwrap_or(Played {
            index: 0,
            repetition: 0,
            of: 1,
            step: super::model::Step::new(Intensity::Work, Length::Open, Target::Free),
        });
        Self {
            workout,
            steps,
            position: 0,
            played,
            step_ms: 0,
            step_mm: 0,
            extra_s: 0,
            extra_m: 0,
            power: Power3s::default(),
            last_sample: Sample::default(),
            summary: Summary {
                steps: steps as u16,
                ..Summary::default()
            },
            test_mj: 0,
            test_ms: 0,
            minute: [0; MINUTE],
            minute_len: 0,
            minute_at: 0,
            minute_rest_ms: 0,
            best_minute: None,
            ramp_low_ms: 0,
            done: false,
        }
    }

    pub fn workout(&self) -> &Workout {
        &self.workout
    }

    pub fn done(&self) -> bool {
        self.done
    }

    pub fn summary(&self) -> Summary {
        self.summary
    }

    /// The played step after `position`.
    fn played_at(&self, position: usize) -> Option<Played> {
        self.workout.played().nth(position)
    }

    fn duration_s(&self) -> Option<u32> {
        match self.played.step.length {
            Length::Time(secs) => Some(u32::from(secs) + self.extra_s),
            _ => None,
        }
    }

    /// The step on now lasts a distance, so its lap ends by distance.
    pub fn distance_step(&self) -> bool {
        self.distance_m().is_some()
    }

    fn distance_m(&self) -> Option<u32> {
        match self.played.step.length {
            Length::Distance(m) => Some(m + self.extra_m),
            _ => None,
        }
    }

    fn reading(&self, metric: Metric) -> Option<u16> {
        match metric {
            Metric::Power => self.power.watts(),
            Metric::Heart => self.last_sample.heart_bpm,
            Metric::Cadence => self.last_sample.cadence_tenths.map(|t| (t + 5) / 10),
        }
    }

    fn band(&self, profile: &TrainingProfile) -> Option<Band> {
        self.played
            .step
            .target
            .band(profile, (self.step_ms / 1000) as u32)
    }

    /// Account `elapsed_ms` of recorded riding over `distance_mm` with
    /// `sample`'s readings. Riding past a step's end counts in the steps
    /// after it. After the last step the readings stay fresh, nothing else.
    pub fn add(
        &mut self,
        elapsed_ms: u64,
        distance_mm: u64,
        sample: &Sample,
        profile: &TrainingProfile,
    ) -> Events {
        let mut events = Events::new();
        if elapsed_ms == 0 {
            return events;
        }
        self.read(sample);
        let (mut ms, mut mm) = (elapsed_ms, distance_mm);
        // Each part ends a step or uses up the update.
        for _ in 0..=MAX_PLAYED {
            if self.done || ms == 0 {
                break;
            }
            let (part_ms, part_mm) = self.until_end(ms, mm);
            ms -= part_ms;
            mm -= part_mm;
            events.extend(self.ride_part(part_ms, part_mm, sample, profile));
        }
        events
    }

    /// Take `sample`'s readings, once per update.
    pub fn read(&mut self, sample: &Sample) {
        self.power.add(sample.power_watts);
        self.last_sample = *sample;
    }

    /// Account part of an update that `until_end` gave, after `read`: it
    /// ends at most the step on now.
    pub fn ride_part(
        &mut self,
        part_ms: u64,
        part_mm: u64,
        sample: &Sample,
        profile: &TrainingProfile,
    ) -> Events {
        let mut events = Events::new();
        if !self.done && part_ms > 0 {
            self.account(part_ms, part_mm, sample, profile, &mut events);
        }
        events
    }

    /// The part of `ms` over `mm` that the step on now still holds; all of
    /// it once the workout is done.
    pub fn until_end(&self, ms: u64, mm: u64) -> (u64, u64) {
        if self.done {
            return (ms, mm);
        }
        if let Some(duration_s) = self.duration_s() {
            let left = (u64::from(duration_s) * 1000).saturating_sub(self.step_ms);
            if ms > left {
                return (left, mm * left / ms);
            }
        } else if let Some(metres) = self.distance_m() {
            let left = (u64::from(metres) * 1000).saturating_sub(self.step_mm);
            if mm > left {
                return ((ms * left).div_ceil(mm), left);
            }
        }
        (ms, mm)
    }

    fn account(
        &mut self,
        elapsed_ms: u64,
        distance_mm: u64,
        sample: &Sample,
        profile: &TrainingProfile,
        events: &mut Events,
    ) {
        if let Some(band) = self.band(profile)
            && let Some(compliance) = self.compliance(band, profile)
        {
            let summary = &mut self.summary;
            summary.targeted_ms += elapsed_ms;
            *match compliance {
                Compliance::Under => &mut summary.under_ms,
                Compliance::In => &mut summary.in_ms,
                Compliance::Over => &mut summary.over_ms,
            } += elapsed_ms;
        }
        let before_ms = self.step_ms;
        self.step_ms += elapsed_ms;
        self.step_mm += distance_mm;
        self.measure(elapsed_ms, sample, profile);
        if let Some(duration_s) = self.duration_s() {
            let end = u64::from(duration_s) * 1000;
            for n in (1..=COUNTDOWN_S).rev() {
                let at = end.saturating_sub(u64::from(n) * 1000);
                if before_ms < at && self.step_ms >= at && self.step_ms < end {
                    let _ = events.push(Event::Countdown(n));
                }
            }
            if self.step_ms >= end {
                self.next(events, false);
            }
        } else if let Some(metres) = self.distance_m() {
            if self.step_mm >= u64::from(metres) * 1000 {
                self.next(events, false);
            }
        } else if self.ramp_failed() {
            self.next(events, false);
        }
    }

    /// A heart zone target holds the whole zone, open ends included; other
    /// targets hold their band.
    fn compliance(&self, band: Band, profile: &TrainingProfile) -> Option<Compliance> {
        let reading = self.reading(band.metric)?;
        if let Target::HeartZone(zone) = self.played.step.target {
            let at = profile.heart_zone(reading);
            return Some(match at.cmp(&zone.clamp(1, 5)) {
                core::cmp::Ordering::Less => Compliance::Under,
                core::cmp::Ordering::Equal => Compliance::In,
                core::cmp::Ordering::Greater => Compliance::Over,
            });
        }
        Some(if reading < band.low {
            Compliance::Under
        } else if reading > band.high {
            Compliance::Over
        } else {
            Compliance::In
        })
    }

    /// The test's numbers: the twenty's energy, the ramp's minutes and
    /// whether the rider is holding the ramp.
    fn measure(&mut self, elapsed_ms: u64, sample: &Sample, profile: &TrainingProfile) {
        if self.played.step.intensity != Intensity::Test {
            return;
        }
        let watts = sample.power_watts.unwrap_or(0);
        match self.workout.test {
            Some(Test::Twenty) => {
                self.test_mj += u64::from(watts) * elapsed_ms;
                self.test_ms += elapsed_ms;
            }
            Some(Test::Ramp) => {
                // One window slot per whole second ridden, however updates fall.
                let total = self.minute_rest_ms + elapsed_ms;
                self.minute_rest_ms = total % 1000;
                for _ in 0..(total / 1000).min(MINUTE as u64) {
                    self.minute[self.minute_at] = watts;
                    self.minute_at = (self.minute_at + 1) % MINUTE;
                    self.minute_len = (self.minute_len + 1).min(MINUTE);
                    if self.minute_len == MINUTE {
                        let sum: u32 = self.minute.iter().map(|&w| u32::from(w)).sum();
                        let mean = ((sum + 30) / MINUTE as u32) as u16;
                        self.best_minute = Some(self.best_minute.map_or(mean, |b| b.max(mean)));
                    }
                }
                let target = self.band(profile).map_or(0, |b| u32::from(b.low));
                // Without a power reading there is no evidence either way.
                match self.power.watts() {
                    Some(w) if u32::from(w) * 100 >= target * RAMP_FAIL_PCT => self.ramp_low_ms = 0,
                    Some(_) => self.ramp_low_ms += elapsed_ms,
                    None => {}
                }
            }
            None => {}
        }
    }

    fn ramp_failed(&self) -> bool {
        matches!(self.played.step.target, Target::Ramp { every, .. }
            if self.step_ms >= u64::from(every) * 1000 && self.ramp_low_ms >= RAMP_FAIL_MS)
    }

    /// The rider ends the step now, as a lap press does.
    pub fn skip(&mut self) -> Events {
        let mut events = Events::new();
        if !self.done {
            self.next(&mut events, true);
        }
        events
    }

    /// Add time (or distance) to the step on now; open steps need none.
    pub fn extend(&mut self) -> bool {
        match self.played.step.length {
            Length::Time(_) if !self.done => self.extra_s += EXTEND_S,
            Length::Distance(_) if !self.done => self.extra_m += EXTEND_M,
            _ => return false,
        }
        true
    }

    fn next(&mut self, events: &mut Events, skipped: bool) {
        // A test result comes from its own step, as that step ends.
        if self.played.step.intensity == Intensity::Test
            && let Some(ftp) = self.result(skipped)
        {
            self.summary.ftp = Some(ftp);
            let _ = events.push(Event::Ftp(ftp));
        }
        self.summary.steps_done += 1;
        self.summary.skipped += u16::from(skipped);
        self.position += 1;
        self.step_ms = 0;
        self.step_mm = 0;
        self.extra_s = 0;
        self.extra_m = 0;
        match self.played_at(self.position) {
            Some(played) => {
                self.played = played;
                let _ = events.push(Event::Started(self.position));
            }
            None => {
                self.done = true;
                self.summary.finished = true;
                let _ = events.push(Event::Finished);
            }
        }
    }

    /// The FTP the test step that is ending measured, if it counts: the
    /// whole twenty minutes, or a ramp with at least one full minute.
    fn result(&self, skipped: bool) -> Option<u16> {
        let ftp = match self.workout.test? {
            Test::Twenty => {
                if skipped || self.test_ms < u64::from(self.duration_s()?) * 1000 {
                    return None;
                }
                let average = (self.test_mj + self.test_ms / 2) / self.test_ms;
                (average * 95 + 50) / 100
            }
            Test::Ramp => (u64::from(self.best_minute?) * 75 + 50) / 100,
        };
        let ftp = u16::try_from(ftp).ok()?;
        TrainingProfile::FTP_WATTS.contains(&ftp).then_some(ftp)
    }

    /// The step on now, for the screen.
    pub fn now(&self, profile: &TrainingProfile) -> Now {
        let band = if self.done { None } else { self.band(profile) };
        let reading = band.map_or(self.power.watts(), |band| self.reading(band.metric));
        let (left, progress) = if let Some(duration) = self.duration_s() {
            let left_ms = (u64::from(duration) * 1000).saturating_sub(self.step_ms);
            let progress = self.step_ms * 1000 / (u64::from(duration) * 1000).max(1);
            (
                Left::Secs(left_ms.div_ceil(1000) as u32),
                progress.min(1000) as u16,
            )
        } else if let Some(metres) = self.distance_m() {
            let done = (self.step_mm / 1000) as u32;
            let progress = self.step_mm / u64::from(metres.max(1));
            (
                Left::Metres(metres.saturating_sub(done)),
                progress.min(1000) as u16,
            )
        } else {
            (Left::Open((self.step_ms / 1000) as u32), 0)
        };
        let test = (self.played.step.intensity == Intensity::Test && !self.done)
            .then(|| match self.workout.test {
                Some(Test::Twenty) => (self.test_ms > 0).then(|| {
                    TestReadout::Average(((self.test_mj + self.test_ms / 2) / self.test_ms) as u16)
                }),
                Some(Test::Ramp) => self.best_minute.map(TestReadout::BestMinute),
                None => None,
            })
            .flatten();
        Now {
            position: self.position.min(self.steps.saturating_sub(1)),
            steps: self.steps,
            played: self.played,
            band,
            reading,
            compliance: band.and_then(|band| self.compliance(band, profile)),
            left,
            progress,
            next: if self.done {
                None
            } else {
                self.played_at(self.position + 1)
            },
            test,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workout::library::{FTP_RAMP, FTP_TWENTY, VO2_MAX};

    fn riding(watts: u16) -> Sample {
        Sample {
            power_watts: Some(watts),
            speed_mm_s: Some(8000),
            ..Sample::default()
        }
    }

    /// Ride `seconds` at `watts`, one-second steps, collecting events.
    fn ride(player: &mut Player, seconds: u32, watts: u16) -> std::vec::Vec<Event> {
        let profile = TrainingProfile::new(250, 170).unwrap();
        let mut events = std::vec::Vec::new();
        for _ in 0..seconds {
            events.extend(player.add(1000, 8000, &riding(watts), &profile));
        }
        events
    }

    #[test]
    fn steps_count_down_advance_and_finish() {
        let mut player = Player::new(VO2_MAX);
        let profile = TrainingProfile::new(250, 170).unwrap();
        assert_eq!(player.now(&profile).steps, 12);
        let events = ride(&mut player, 12 * 60 - 4, 150);
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(player.now(&profile).left, Left::Secs(4));
        let events = ride(&mut player, 4, 150);
        assert_eq!(
            events,
            [
                Event::Countdown(3),
                Event::Countdown(2),
                Event::Countdown(1),
                Event::Started(1)
            ]
        );
        let now = player.now(&profile);
        assert_eq!(now.played.index, 1);
        assert_eq!(now.played.repetition, 0);
        assert_eq!(now.left, Left::Secs(180));
        // 106-120 % of 250 W.
        assert_eq!(now.band.map(|b| (b.low, b.high)), Some((265, 300)));
        assert_eq!(
            now.next.map(|p| p.step.intensity),
            Some(Intensity::Recovery)
        );
        // Riding through everything else ends the workout once.
        let events = ride(&mut player, 5 * 360 + 600, 280);
        assert_eq!(events.iter().filter(|e| **e == Event::Finished).count(), 1);
        assert!(player.done());
        assert!(ride(&mut player, 60, 280).is_empty());
        let summary = player.summary();
        assert_eq!(
            (summary.steps_done, summary.steps, summary.skipped),
            (12, 12, 0)
        );
        assert!(summary.finished);
    }

    #[test]
    fn compliance_uses_three_second_power() {
        let mut player = Player::new(VO2_MAX);
        let profile = TrainingProfile::new(250, 170).unwrap();
        player.skip();
        ride(&mut player, 3, 280);
        let now = player.now(&profile);
        assert_eq!(
            (now.reading, now.compliance),
            (Some(280), Some(Compliance::In))
        );
        // One low second moves the average, not the verdict on its own.
        ride(&mut player, 1, 220);
        let now = player.now(&profile);
        assert_eq!(now.reading, Some(260));
        assert_eq!(now.compliance, Some(Compliance::Under));
        ride(&mut player, 3, 320);
        assert_eq!(player.now(&profile).compliance, Some(Compliance::Over));
        let summary = player.summary();
        assert_eq!(summary.targeted_ms, 7000);
        assert_eq!(summary.in_ms + summary.under_ms + summary.over_ms, 7000);
    }

    #[test]
    fn skip_and_extend_change_the_step() {
        let mut player = Player::new(VO2_MAX);
        let profile = TrainingProfile::default();
        assert_eq!(player.skip().as_slice(), [Event::Started(1)]);
        assert!(player.extend());
        assert_eq!(player.now(&profile).left, Left::Secs(240));
        ride(&mut player, 200, 250);
        assert_eq!(player.now(&profile).played.index, 1, "extended past 3 min");
        ride(&mut player, 40, 250);
        assert_eq!(player.now(&profile).played.index, 2);
        assert_eq!(
            player.now(&profile).left,
            Left::Secs(180),
            "extension ends with its step"
        );
        assert_eq!(player.summary().skipped, 1);
    }

    #[test]
    fn twenty_minute_test_offers_ninety_five_percent() {
        let mut player = Player::new(FTP_TWENTY);
        let profile = TrainingProfile::default();
        // Skip to the measured step.
        while player.now(&profile).played.step.intensity != Intensity::Test {
            player.skip();
        }
        ride(&mut player, 10 * 60, 250);
        assert_eq!(player.now(&profile).test, Some(TestReadout::Average(250)));
        let events = ride(&mut player, 10 * 60, 270);
        assert!(events.contains(&Event::Ftp(247)), "{events:?}");
        assert_eq!(player.summary().ftp, Some(247));
        assert_eq!(
            player.now(&profile).played.step.intensity,
            Intensity::Cooldown
        );
    }

    #[test]
    fn a_cut_short_twenty_minutes_measures_nothing() {
        let mut player = Player::new(FTP_TWENTY);
        let profile = TrainingProfile::default();
        while player.now(&profile).played.step.intensity != Intensity::Test {
            player.skip();
        }
        ride(&mut player, 600, 300);
        let events = player.skip();
        assert!(!events.iter().any(|e| matches!(e, Event::Ftp(_))));
        assert_eq!(player.summary().ftp, None);
    }

    #[test]
    fn ramp_ends_at_failure_and_offers_three_quarters_of_the_best_minute() {
        let mut player = Player::new(FTP_RAMP);
        let profile = TrainingProfile::default();
        player.skip();
        // The rider holds each step for 8 minutes: 100..240 W.
        for stage in 0..8u16 {
            let events = ride(&mut player, 60, 100 + 20 * stage);
            assert!(events.is_empty(), "{stage}: {events:?}");
        }
        let now = player.now(&profile);
        assert_eq!(now.band.map(|b| b.low), Some(260));
        assert_eq!(now.test, Some(TestReadout::BestMinute(240)));
        // Then fades: 200 W is under 90 % of 260 W, and the ramp ends 10 s later.
        let mut events = std::vec::Vec::new();
        for _ in 0..20 {
            events.extend(ride(&mut player, 1, 200));
            if !events.is_empty() {
                break;
            }
        }
        // 75 % of 240 W.
        assert_eq!(events, [Event::Ftp(180), Event::Started(2)]);
        assert_eq!(
            player.now(&profile).played.step.intensity,
            Intensity::Cooldown
        );
    }

    #[test]
    fn a_ramp_stopped_by_the_rider_still_counts() {
        let mut player = Player::new(FTP_RAMP);
        player.skip();
        ride(&mut player, 30, 100);
        assert_eq!(
            player.skip().as_slice(),
            [Event::Started(2)],
            "no full minute yet"
        );
        let mut player = Player::new(FTP_RAMP);
        player.skip();
        for stage in 0..10u16 {
            ride(&mut player, 60, 100 + 20 * stage);
        }
        assert_eq!(
            player.skip().as_slice(),
            [Event::Ftp(210), Event::Started(2)]
        );
    }

    #[test]
    fn power_three_seconds_ignores_gaps() {
        let mut power = Power3s::default();
        assert_eq!(power.watts(), None);
        power.add(Some(300));
        power.add(None);
        assert_eq!(power.watts(), Some(300));
        power.add(Some(201));
        assert_eq!(power.watts(), Some(251));
        power.add(None);
        power.add(None);
        power.add(None);
        assert_eq!(power.watts(), None);
    }

    #[test]
    fn riding_past_a_step_end_counts_in_the_next_steps() {
        use crate::workout::model::Step;
        let profile = TrainingProfile::default();
        let short = Workout::new(
            "SHORT",
            &[
                Step::new(Intensity::Work, Length::Time(1), Target::Free),
                Step::new(Intensity::Work, Length::Time(1), Target::Free),
                Step::new(Intensity::Work, Length::Distance(100), Target::Free),
                Step::new(Intensity::Work, Length::Time(60), Target::Free),
            ],
            &[],
            None,
        );
        let mut player = Player::new(short);
        // Three seconds over 300 m end both one-second steps and the 100 m one.
        let events = player.add(3000, 300_000, &riding(100), &profile);
        assert_eq!(
            events.as_slice(),
            [Event::Started(1), Event::Started(2), Event::Started(3)]
        );
        let events = player.add(1000, 100_000, &riding(100), &profile);
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(player.now(&profile).left, Left::Secs(59));
    }

    #[test]
    fn the_ramp_minute_follows_ridden_time_not_updates() {
        let profile = TrainingProfile::default();
        let mut player = Player::new(FTP_RAMP);
        player.skip();
        // Twenty three-second updates are one full minute.
        for _ in 0..20 {
            player.add(3000, 24_000, &riding(110), &profile);
        }
        assert_eq!(
            player.now(&profile).test,
            Some(TestReadout::BestMinute(110))
        );
        assert_eq!(
            player.skip().as_slice(),
            [Event::Ftp(83), Event::Started(2)]
        );
    }

    #[test]
    fn readings_stay_fresh_after_the_last_step() {
        let profile = TrainingProfile::default();
        let mut player = Player::new(VO2_MAX);
        for _ in 0..12 {
            player.skip();
        }
        assert!(player.done());
        let before = player.summary();
        ride(&mut player, 5, 180);
        assert_eq!(player.now(&profile).reading, Some(180));
        assert_eq!(player.summary(), before, "no statistics after the end");
    }

    #[test]
    fn a_heart_zone_target_holds_its_open_ends() {
        use crate::workout::model::Step;
        let profile = TrainingProfile::default();
        let check = |zone: u8, bpm: u16| {
            let steps = [Step::new(
                Intensity::Work,
                Length::Time(60),
                Target::HeartZone(zone),
            )];
            let mut player = Player::new(Workout::new("HR", &steps, &[], None));
            let sample = Sample {
                heart_bpm: Some(bpm),
                ..Sample::default()
            };
            player.add(1000, 0, &sample, &profile);
            player.now(&profile).compliance
        };
        assert_eq!(check(1, 60), Some(Compliance::In));
        assert_eq!(check(5, 220), Some(Compliance::In));
        assert_eq!(check(1, 180), Some(Compliance::Over));
        assert_eq!(check(5, 100), Some(Compliance::Under));
    }

    #[test]
    fn a_power_dropout_does_not_end_the_ramp() {
        let profile = TrainingProfile::default();
        let mut player = Player::new(FTP_RAMP);
        player.skip();
        ride(&mut player, 90, 150);
        let silent = Sample {
            speed_mm_s: Some(8000),
            ..Sample::default()
        };
        for _ in 0..30 {
            assert!(player.add(1000, 8000, &silent, &profile).is_empty());
        }
        assert_eq!(player.now(&profile).played.index, 1, "still ramping");
    }
}
