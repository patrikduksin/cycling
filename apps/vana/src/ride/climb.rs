//! Climb detection and the recent elevation profile, from the smoothed
//! barometric altitude `Totals` keeps and the wheel distance; no maps.
//!
//! The caller feeds the same recorded seconds as `Totals`, after it, so a
//! pause adds nothing. Altitude is sampled every `STEP_MM` of distance
//! (interpolated between seconds); detection runs on those samples:
//!
//! - A climb starts when the grade over the last 200 m is at least 3%, from
//!   the last lowest point of those 200 m.
//! - It is confirmed, and shown, once it has gained 20 m. A start that never
//!   gains 20 m is forgotten.
//! - It ends when the grade over the last 200 m falls below 1%, or 10 m below
//!   its peak. A finished climb is measured from its start to its peak, and
//!   the next climb is looked for only in the road after it ends.
//!
//! A step without a fresh altitude is a gap, not a level road: the profile
//! bridges it, but detection starts its 200 m window afresh after it.
//!
//! Integer math and fixed arrays only: about 1 KB in all.

/// Distance between profile samples (20 m).
pub const STEP_MM: u64 = 20_000;
/// Profile samples kept: the last 4 km.
pub const PROFILE_LEN: usize = 200;
/// Profile samples spanning the detection window (200 m).
const WINDOW: usize = 10;
/// A profile column's grade spans this many samples each side (100 m), so
/// it is a 200 m grade centred on the column.
const LOCAL: usize = 5;
/// A climb starts at this grade over the window, in tenths of a percent.
pub const START_TENTHS: i32 = 30;
/// A climb ends below this grade over the window.
pub const END_TENTHS: i32 = 10;
/// Gain that confirms a climb (20 m).
pub const CONFIRM_DM: i32 = 200;
/// Drop below the peak that ends a climb (10 m).
pub const DROP_DM: i32 = 100;
/// VAM looks back over this many recorded steps (about 60 s).
const VAM_LEN: usize = 60;
/// VAM needs at least this much time in its window, in tenths of a second.
const VAM_MIN_DS: u16 = 300;

/// How steep a grade is, in the bands the climb page colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Steepness {
    /// Down by 1.5% or more.
    Descent,
    /// Flat or rolling: from -1.5% to 3%.
    #[default]
    Easy,
    /// 3% to 6%.
    Moderate,
    /// 6% to 9%.
    Steep,
    /// 9% to 12%.
    Hard,
    /// 12% and more.
    Severe,
}

impl Steepness {
    const ALL: [Self; 6] = [
        Self::Descent,
        Self::Easy,
        Self::Moderate,
        Self::Steep,
        Self::Hard,
        Self::Severe,
    ];
    /// Where each band after `Descent` begins, in tenths of a percent.
    const FROM: [i32; 5] = [-15, 30, 60, 90, 120];
    /// A band holds until the grade falls this far below it (0.5%); a
    /// steeper grade moves up at once, so a steady 6% reads Steep.
    const HYSTERESIS: i32 = 5;

    pub fn of(tenths: i32) -> Self {
        Self::ALL[Self::FROM.iter().filter(|&&from| tenths >= from).count()]
    }

    /// The band for `tenths` next to a stretch in `self`: steeper grades
    /// move up at their boundary, but it only drops back once the grade is
    /// clearly below `self`, so a steady grade on a boundary keeps one colour.
    pub fn after(self, tenths: i32) -> Self {
        let index = self as usize;
        let low = index
            .checked_sub(1)
            .map_or(i32::MIN, |below| Self::FROM[below] - Self::HYSTERESIS);
        let high = Self::FROM.get(index).copied().unwrap_or(i32::MAX);
        if (low..high).contains(&tenths) {
            self
        } else {
            Self::of(tenths)
        }
    }
}

/// Where the altitude was at a recorded distance and time.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Point {
    altitude_dm: i32,
    distance_mm: u64,
    time_ms: u64,
}

impl Point {
    /// The point at `distance_mm` on the straight line from `self` to `to`.
    fn toward(self, to: Point, distance_mm: u64) -> Point {
        let span = to.distance_mm.saturating_sub(self.distance_mm);
        if span == 0 {
            return to;
        }
        let into = distance_mm.saturating_sub(self.distance_mm).min(span);
        let rise = i64::from(to.altitude_dm - self.altitude_dm);
        Point {
            altitude_dm: self.altitude_dm + (rise * into as i64 / span as i64) as i32,
            distance_mm,
            time_ms: self.time_ms + (to.time_ms - self.time_ms) * into / span,
        }
    }
}

/// Grade from `from` to `to` in tenths of a percent; none without distance.
fn grade_tenths(from: Point, to: Point) -> Option<i32> {
    let run = to.distance_mm.checked_sub(from.distance_mm)?;
    (run != 0).then(|| (i64::from(to.altitude_dm - from.altitude_dm) * 100_000 / run as i64) as i32)
}

/// A climb's measurements: finished, or so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Distance from the first step fed to where the climb starts.
    pub start_mm: u64,
    pub start_altitude_dm: i32,
    /// From the start to the highest point.
    pub gain_dm: u32,
    pub length_mm: u64,
    /// Recorded time on the climb.
    pub time_ms: u64,
}

impl Summary {
    /// Average grade, in tenths of a percent.
    pub fn grade_tenths(&self) -> Option<i32> {
        (self.length_mm != 0).then(|| (u64::from(self.gain_dm) * 100_000 / self.length_mm) as i32)
    }

    /// Its category, from length and average grade.
    pub fn category(&self) -> Category {
        Category::of(self.gain_dm)
    }

    /// Average vertical ascent speed, in metres per hour.
    pub fn vam_m_h(&self) -> Option<u32> {
        (self.time_ms != 0).then(|| (u64::from(self.gain_dm) * 360_000 / self.time_ms) as u32)
    }
}

/// How hard a climb is, as cycling computers and Strava grade them: its
/// score is length in metres times average grade in percent, which is its
/// gain in metres times 100. Cat 4 from 8,000 (80 m of gain), Cat 3 from
/// 16,000, Cat 2 from 32,000, Cat 1 from 48,000 and HC from 64,000. A
/// confirmed climb below Cat 4 is still a climb, just uncategorised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Uncategorised,
    Four,
    Three,
    Two,
    One,
    Hors,
}

impl Category {
    /// Gain where each category after `Uncategorised` begins, decimetres.
    const FROM_DM: [u32; 5] = [800, 1_600, 3_200, 4_800, 6_400];

    pub fn of(gain_dm: u32) -> Self {
        const ALL: [Category; 6] = [
            Category::Uncategorised,
            Category::Four,
            Category::Three,
            Category::Two,
            Category::One,
            Category::Hors,
        ];
        ALL[Self::FROM_DM
            .iter()
            .filter(|&&from| gain_dm >= from)
            .count()]
    }

    /// Its short name: `CAT 4` to `CAT 1`, `HC`, or `CLIMB`.
    pub fn name(self) -> &'static [u8] {
        match self {
            Self::Uncategorised => b"CLIMB",
            Self::Four => b"CAT 4",
            Self::Three => b"CAT 3",
            Self::Two => b"CAT 2",
            Self::One => b"CAT 1",
            Self::Hors => b"HC",
        }
    }
}

/// What a recorded step changed about climbing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A climb gained enough to count; it is shown from now on.
    Confirmed,
    /// The climb ended, with its final measurements.
    Finished(Summary),
}

#[derive(Clone, Copy, Debug)]
struct Climb {
    start: Point,
    peak: Point,
    confirmed: bool,
}

impl Climb {
    fn summary(&self, distance_mm: u64, time_ms: u64) -> Summary {
        Summary {
            start_mm: self.start.distance_mm,
            start_altitude_dm: self.start.altitude_dm,
            gain_dm: (self.peak.altitude_dm - self.start.altitude_dm).max(0) as u32,
            length_mm: distance_mm.saturating_sub(self.start.distance_mm),
            time_ms: time_ms.saturating_sub(self.start.time_ms),
        }
    }
}

/// A fixed ring that keeps the newest `N` values.
#[derive(Clone, Copy, Debug)]
struct Ring<T: Copy + Default, const N: usize> {
    values: [T; N],
    len: usize,
    next: usize,
}

impl<T: Copy + Default, const N: usize> Default for Ring<T, N> {
    fn default() -> Self {
        Self {
            values: [T::default(); N],
            len: 0,
            next: 0,
        }
    }
}

impl<T: Copy + Default, const N: usize> Ring<T, N> {
    fn push(&mut self, value: T) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % N;
        self.len = (self.len + 1).min(N);
    }

    /// The `index`th oldest value.
    fn get(&self, index: usize) -> T {
        self.values[(self.next + N - self.len + index) % N]
    }

    fn full(&self) -> bool {
        self.len == N
    }

    fn iter(&self) -> impl Iterator<Item = T> + '_ {
        (0..self.len).map(|index| self.get(index))
    }
}

/// One profile sample, as the climb page draws it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProfileSample {
    /// Relative to a base near the rider, in decimetres; compare samples
    /// with each other only.
    pub altitude_dm: i16,
    /// Grade over up to 200 m centred on this sample (only the 100 m before
    /// it at the newest end), in tenths of a percent.
    pub grade_tenths: i16,
}

#[derive(Clone, Debug, Default)]
pub struct Climbs {
    distance_mm: u64,
    time_ms: u64,
    /// The latest altitude reading, which later samples interpolate from.
    latest: Option<Point>,
    /// The distance of the next profile sample.
    next_mm: u64,
    /// Profile and VAM altitudes are stored relative to this, to fit `i16`;
    /// it moves with the rider (see `rebase`).
    base_dm: i32,
    /// Steps went by without a fresh altitude since the latest reading.
    gap: bool,
    /// Altitude every `STEP_MM`, relative to `base_dm`.
    profile: Ring<i16, PROFILE_LEN>,
    /// The last 200 m of samples, for detection.
    window: Ring<Point, { WINDOW + 1 }>,
    /// Relative altitude and recorded time (wrapping tenths of a second) of
    /// the last recorded steps.
    vam: Ring<(i16, u16), { VAM_LEN + 1 }>,
    current: Option<Climb>,
    last: Option<Summary>,
}

impl Climbs {
    /// Account one recorded step of `elapsed_ms` that covered `distance_mm`
    /// and ended at the smoothed `altitude_dm`, if there is a barometer.
    pub fn add(
        &mut self,
        elapsed_ms: u64,
        distance_mm: u64,
        altitude_dm: Option<i32>,
    ) -> Option<Event> {
        self.distance_mm = self.distance_mm.saturating_add(distance_mm);
        self.time_ms = self.time_ms.saturating_add(elapsed_ms);
        let Some(altitude_dm) = altitude_dm else {
            self.gap = self.latest.is_some();
            return None;
        };
        let now = Point {
            altitude_dm,
            distance_mm: self.distance_mm,
            time_ms: self.time_ms,
        };
        let Some(previous) = self.latest.replace(now) else {
            // The first reading starts the profile right here.
            self.base_dm = altitude_dm;
            self.next_mm = now.distance_mm;
            self.vam.push((0, (now.time_ms / 100) as u16));
            return self.sample_due(now, now, true);
        };
        self.rebase(altitude_dm);
        self.vam
            .push((self.relative(altitude_dm), (now.time_ms / 100) as u16));
        if core::mem::take(&mut self.gap) {
            // Detection never spans a gap; the profile bridges it in a line.
            self.window = Ring::default();
            return self.sample_due(previous, now, false);
        }
        self.sample_due(previous, now, true)
    }

    /// Keep stored altitudes near zero: past half the `i16` range from the
    /// base, the base moves to `altitude_dm` and every stored value shifts.
    /// The profile and VAM window span far less than the remaining range.
    fn rebase(&mut self, altitude_dm: i32) {
        const REACH: i32 = i16::MAX as i32 / 2;
        let offset = altitude_dm - self.base_dm;
        if offset.abs() <= REACH {
            return;
        }
        self.base_dm = altitude_dm;
        let shift = |value: i16| {
            (i32::from(value) - offset).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
        };
        for value in &mut self.profile.values {
            *value = shift(*value);
        }
        for (value, _) in &mut self.vam.values {
            *value = shift(*value);
        }
    }

    fn relative(&self, altitude_dm: i32) -> i16 {
        (altitude_dm - self.base_dm).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    }

    /// Sample every step boundary between `previous` and `now`; only into
    /// the profile unless `detect`.
    fn sample_due(&mut self, previous: Point, now: Point, detect: bool) -> Option<Event> {
        let mut event = None;
        while self.next_mm <= now.distance_mm {
            let point = previous.toward(now, self.next_mm);
            if detect {
                event = self.sample(point).or(event);
            } else {
                self.profile.push(self.relative(point.altitude_dm));
            }
            self.next_mm += STEP_MM;
        }
        event
    }

    fn sample(&mut self, point: Point) -> Option<Event> {
        self.profile.push(self.relative(point.altitude_dm));
        self.window.push(point);
        let grade = self
            .window
            .full()
            .then(|| grade_tenths(self.window.get(0), point))
            .flatten();
        if self.current.is_none() {
            if grade.is_none_or(|grade| grade < START_TENTHS) {
                return None;
            }
            let start = self
                .window
                .iter()
                .min_by_key(|p| (p.altitude_dm, core::cmp::Reverse(p.distance_mm)))
                .unwrap_or(point);
            let peak = self
                .window
                .iter()
                .filter(|p| p.distance_mm >= start.distance_mm)
                .max_by_key(|p| p.altitude_dm)
                .unwrap_or(point);
            self.current = Some(Climb {
                start,
                peak,
                confirmed: false,
            });
        }
        let climb = self.current.as_mut()?;
        if point.altitude_dm > climb.peak.altitude_dm {
            climb.peak = point;
        }
        let mut event = None;
        if !climb.confirmed && climb.peak.altitude_dm - climb.start.altitude_dm >= CONFIRM_DM {
            climb.confirmed = true;
            event = Some(Event::Confirmed);
        }
        let flattened = grade.is_some_and(|grade| grade < END_TENTHS);
        let dropped = point.altitude_dm <= climb.peak.altitude_dm - DROP_DM;
        if flattened || dropped {
            let climb = *climb;
            self.current = None;
            // The road this climb used cannot start or end another one.
            self.window = Ring::default();
            self.window.push(point);
            if climb.confirmed {
                let summary = climb.summary(climb.peak.distance_mm, climb.peak.time_ms);
                self.last = Some(summary);
                event = Some(Event::Finished(summary));
            }
        }
        event
    }

    /// The confirmed climb under way, measured from its start to here.
    pub fn climbing(&self) -> Option<Summary> {
        self.current
            .filter(|climb| climb.confirmed)
            .map(|climb| climb.summary(self.distance_mm, self.time_ms))
    }

    /// The last finished climb.
    pub fn last_climb(&self) -> Option<Summary> {
        self.last
    }

    /// Vertical ascent speed over about the last minute, in metres per hour;
    /// none before 30 s of altitude. Never negative.
    pub fn vam_m_h(&self) -> Option<u32> {
        if self.vam.len < 2 {
            return None;
        }
        let (from, then) = self.vam.get(0);
        let (to, now) = self.vam.get(self.vam.len - 1);
        let span = now.wrapping_sub(then);
        (span >= VAM_MIN_DS)
            .then(|| (i32::from(to) - i32::from(from)).max(0) as u32 * 3600 / u32::from(span))
    }

    /// The profile, oldest first, into `out`; returns how many samples it has.
    pub fn profile(&self, out: &mut [ProfileSample; PROFILE_LEN]) -> usize {
        let len = self.profile.len;
        for (index, sample) in out.iter_mut().enumerate().take(len) {
            let altitude = self.profile.get(index);
            let from = index.saturating_sub(LOCAL);
            let to = (index + LOCAL).min(len - 1);
            let rise = i32::from(self.profile.get(to)) - i32::from(self.profile.get(from));
            // A step is 20 m = 200 dm, so tenths of a percent = rise * 5 / steps.
            let grade = if to == from {
                0
            } else {
                rise * 5 / (to - from) as i32
            };
            *sample = ProfileSample {
                altitude_dm: altitude,
                grade_tenths: grade.clamp(-999, 999) as i16,
            };
        }
        len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Feed one-second steps at `speed_mm_s` along `altitude(distance_mm)`,
    /// for `metres`, returning every event.
    fn ride(
        climbs: &mut Climbs,
        travelled: &mut u64,
        metres: u64,
        speed_mm_s: u64,
        altitude: impl Fn(u64) -> i32,
    ) -> Vec<(u64, Event)> {
        let end = *travelled + metres * 1000;
        let mut events = Vec::new();
        while *travelled < end {
            *travelled += speed_mm_s;
            if let Some(event) = climbs.add(1000, speed_mm_s, Some(altitude(*travelled))) {
                events.push((*travelled, event));
            }
        }
        events
    }

    /// Flat for 1 km, then 1.5 km at `grade` percent, then down at the same grade.
    fn hill(grade: i32) -> impl Fn(u64) -> i32 {
        move |mm| {
            let metres = mm as i32 / 1000;
            let up = (metres - 1000).clamp(0, 1500);
            let down = (metres - 2500).clamp(0, 1500);
            5000 + (up - down) * grade / 10
        }
    }

    #[test]
    fn steepness_bands_match_the_research() {
        for (tenths, band) in [
            (-80, Steepness::Descent),
            (-15, Steepness::Easy),
            (29, Steepness::Easy),
            (30, Steepness::Moderate),
            (59, Steepness::Moderate),
            (60, Steepness::Steep),
            (89, Steepness::Steep),
            (90, Steepness::Hard),
            (119, Steepness::Hard),
            (120, Steepness::Severe),
        ] {
            assert_eq!(Steepness::of(tenths), band, "{tenths}");
        }
    }

    #[test]
    fn a_band_holds_until_the_grade_is_clearly_past_its_boundary() {
        // A steady 6% that reads 5.7 to 6.4 keeps one colour.
        let mut band = Steepness::of(61);
        for tenths in [59, 57, 64, 58, 60, 56] {
            band = band.after(tenths);
            assert_eq!(band, Steepness::Steep, "{tenths}");
        }
        assert_eq!(band.after(54), Steepness::Moderate);
        // Up a band right at its boundary.
        assert_eq!(Steepness::Moderate.after(59), Steepness::Moderate);
        assert_eq!(Steepness::Moderate.after(60), Steepness::Steep);
        // Far jumps go straight to their band.
        assert_eq!(Steepness::Easy.after(130), Steepness::Severe);
        assert_eq!(Steepness::Severe.after(-60), Steepness::Descent);
        assert_eq!(Steepness::Easy.after(-20), Steepness::Easy);
        assert_eq!(Steepness::Easy.after(-21), Steepness::Descent);
    }

    #[test]
    fn a_six_percent_hill_is_found_measured_and_kept() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        // 25 km/h on the flat: nothing.
        assert!(ride(&mut climbs, &mut at, 1000, 7000, hill(6)).is_empty());
        assert_eq!(climbs.climbing(), None);
        // 12 km/h up: confirmed once 20 m are gained, about 333 m in.
        let up = ride(&mut climbs, &mut at, 1500, 3333, hill(6));
        assert_eq!(up.len(), 1, "{up:?}");
        let (confirmed_at, event) = up[0];
        assert_eq!(event, Event::Confirmed);
        assert!(
            (1_320_000..1_380_000).contains(&confirmed_at),
            "{confirmed_at}"
        );
        let so_far = climbs.climbing().unwrap();
        assert_eq!(so_far.start_altitude_dm, 5000);
        assert!((880..=900).contains(&so_far.gain_dm), "{so_far:?}");
        assert!(
            (1_480_000..=1_520_000).contains(&so_far.length_mm),
            "{so_far:?}"
        );
        assert!((58..=61).contains(&so_far.grade_tenths().unwrap()));
        // 0.2 m/s up is 720 m/h.
        let vam = climbs.vam_m_h().unwrap();
        assert!((680..=740).contains(&vam), "{vam}");

        // Down at 40 km/h: over the top the last 200 m average under 1% within
        // about 100 m, before the road is 10 m below the peak.
        let down = ride(&mut climbs, &mut at, 1500, 11_111, hill(6));
        assert_eq!(down.len(), 1, "{down:?}");
        let (ended_at, Event::Finished(summary)) = down[0] else {
            panic!("{down:?}");
        };
        assert!((2_560_000..2_620_000).contains(&ended_at), "{ended_at}");
        assert_eq!(climbs.climbing(), None);
        assert_eq!(climbs.last_climb(), Some(summary));
        assert_eq!(summary.start_altitude_dm, 5000);
        assert!((880..=900).contains(&summary.gain_dm), "{summary:?}");
        assert!(
            (1_480_000..=1_520_000).contains(&summary.length_mm),
            "{summary:?}"
        );
        assert!(
            (58..=60).contains(&summary.grade_tenths().unwrap()),
            "{summary:?}"
        );
        // 450 s on the climb, give or take a sample.
        assert!(
            (445_000..=455_000).contains(&summary.time_ms),
            "{summary:?}"
        );
        assert!((700..=730).contains(&summary.vam_m_h().unwrap()));
        assert_eq!(climbs.vam_m_h(), Some(0), "descending");
    }

    #[test]
    fn a_climb_ends_two_hundred_metres_after_it_flattens() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        // 500 m at 5% (25 m), then flat.
        let road = |mm: u64| (mm as i32 / 1000).clamp(0, 500) / 2;
        let events = ride(&mut climbs, &mut at, 1000, 5000, road);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].1, Event::Confirmed);
        let (ended_at, Event::Finished(summary)) = events[1] else {
            panic!("{events:?}");
        };
        // Under 1% over 200 m once less than 40 m of the window still climbs.
        assert!((660_000..=700_000).contains(&ended_at), "{ended_at}");
        assert!((245..=250).contains(&summary.gain_dm), "{summary:?}");
        assert!(
            (490_000..=510_000).contains(&summary.length_mm),
            "{summary:?}"
        );
    }

    #[test]
    fn short_rises_and_gentle_roads_are_not_climbs() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        // 300 m at 5% gains only 15 m, then flat.
        let ramp = |mm: u64| (mm as i32 / 1000).clamp(0, 300) / 2;
        assert!(ride(&mut climbs, &mut at, 1000, 5000, ramp).is_empty());
        // 2 km at 2.5% never starts one.
        let mut climbs = Climbs::default();
        let mut at = 0;
        let gentle = |mm: u64| (mm as i32 / 1000) / 4;
        assert!(ride(&mut climbs, &mut at, 2000, 5000, gentle).is_empty());
        assert_eq!(climbs.last_climb(), None);
        // Neither does a road without a barometer.
        let mut climbs = Climbs::default();
        for _ in 0..1000 {
            assert_eq!(climbs.add(1000, 5000, None), None);
        }
        assert_eq!(
            climbs.profile(&mut [ProfileSample::default(); PROFILE_LEN]),
            0
        );
        assert_eq!(climbs.vam_m_h(), None);
    }

    #[test]
    fn a_dip_mid_climb_does_not_end_it() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        // Up 30 m, down 2 m over 40 m, up 30 m more, then down for good.
        let road = |mm: u64| {
            let m = mm as i32 / 1000;
            match m {
                ..500 => m * 6 / 10,
                500..540 => 300 - (m - 500) / 2,
                540..1040 => 280 + (m - 540) * 6 / 10,
                _ => 580 - (m - 1040) / 2,
            }
        };
        let events = ride(&mut climbs, &mut at, 1500, 4000, road);
        let kinds: Vec<_> = events.iter().map(|(_, e)| *e).collect();
        assert_eq!(kinds.len(), 2, "{events:?}");
        let Event::Finished(summary) = kinds[1] else {
            panic!("{events:?}");
        };
        assert!((575..=580).contains(&summary.gain_dm), "{summary:?}");
    }

    #[test]
    fn a_sharp_drop_below_the_peak_ends_the_climb_before_it_averages_flat() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        // 400 m at 10% (40 m), then a 20% plunge.
        let road = |mm: u64| {
            let m = mm as i32 / 1000;
            if m < 400 { m } else { 400 - (m - 400) * 2 }
        };
        let events = ride(&mut climbs, &mut at, 600, 4000, road);
        assert_eq!(events.len(), 2, "{events:?}");
        let (ended_at, Event::Finished(summary)) = events[1] else {
            panic!("{events:?}");
        };
        // 10 m down takes 50 m; the 200 m average is still 2.5% there.
        assert!((440_000..=480_000).contains(&ended_at), "{ended_at}");
        assert!((385..=400).contains(&summary.gain_dm), "{summary:?}");
    }

    #[test]
    fn a_climb_ended_by_a_drop_is_kept_and_not_reused_by_the_next_samples() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        // 1 km at 20% up, then 20% down, at 2 m/s.
        let road = |mm: u64| {
            let m = mm as i32 / 1000;
            if m < 1000 {
                m * 2
            } else {
                2000 - (m - 1000) * 2
            }
        };
        let events = ride(&mut climbs, &mut at, 2000, 2000, road);
        let kinds: Vec<_> = events.iter().map(|(_, e)| *e).collect();
        assert_eq!(kinds.len(), 2, "{events:?}");
        let Event::Finished(summary) = kinds[1] else {
            panic!("{events:?}");
        };
        assert!(summary.gain_dm >= 1950, "{summary:?}");
        assert!(summary.length_mm >= 980_000, "{summary:?}");
        assert_eq!(climbs.last_climb(), Some(summary));
    }

    #[test]
    fn missing_altitude_is_a_gap_not_a_flat_road() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        let road = |mm: u64| (mm as i32 / 1000) * 6 / 10;
        let events = ride(&mut climbs, &mut at, 800, 4000, road);
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(climbs.climbing().is_some());
        // 400 m without a barometer reading: the climb goes on.
        for _ in 0..100 {
            at += 4000;
            assert_eq!(climbs.add(1000, 4000, None), None);
        }
        assert!(climbs.climbing().is_some());
        // The reading returns higher up; nothing ends, and the profile
        // bridges the gap without a flat step.
        let events = ride(&mut climbs, &mut at, 400, 4000, road);
        assert!(events.is_empty(), "{events:?}");
        let climb = climbs.climbing().unwrap();
        assert!((940..=960).contains(&climb.gain_dm), "{climb:?}");
        let mut profile = [ProfileSample::default(); PROFILE_LEN];
        let len = climbs.profile(&mut profile);
        assert!(
            profile[..len]
                .iter()
                .skip(5)
                .all(|s| (55..=65).contains(&s.grade_tenths)),
            "{profile:?}"
        );
    }

    #[test]
    fn profile_and_vam_stay_right_far_above_the_first_reading() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        // 10% for 50 km: 5 km of climbing, beyond the i16 decimetre range.
        ride(&mut climbs, &mut at, 50_000, 5000, |mm| mm as i32 / 1000);
        let mut profile = [ProfileSample::default(); PROFILE_LEN];
        assert_eq!(climbs.profile(&mut profile), PROFILE_LEN);
        assert!(profile.iter().all(|s| s.grade_tenths == 100), "{profile:?}");
        let newest = i32::from(profile[PROFILE_LEN - 1].altitude_dm);
        assert_eq!(newest - i32::from(profile[0].altitude_dm), 199 * 20);
        // 0.5 m/s up is 1800 m/h.
        assert_eq!(climbs.vam_m_h(), Some(1800));
        assert!(climbs.climbing().unwrap().gain_dm >= 49_000);
    }

    #[test]
    fn profile_samples_every_twenty_metres_and_keeps_four_km() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        let mut profile = [ProfileSample::default(); PROFILE_LEN];
        // Fast steps cross several boundaries; samples interpolate between them.
        ride(&mut climbs, &mut at, 100, 10_000, |mm| mm as i32 / 1000);
        // The first reading, at 10 m, starts the profile.
        assert_eq!(climbs.profile(&mut profile), 5);
        assert_eq!(profile[0].altitude_dm, 0);
        assert_eq!(profile[1].altitude_dm, 20);
        assert_eq!(profile[4].altitude_dm, 80);
        // 10% over the samples so far, centred where they reach.
        assert_eq!(profile[4].grade_tenths, 100);
        assert_eq!(profile[0].grade_tenths, 100);
        ride(&mut climbs, &mut at, 5000, 10_000, |mm| mm as i32 / 1000);
        assert_eq!(climbs.profile(&mut profile), PROFILE_LEN);
        let newest = profile[PROFILE_LEN - 1].altitude_dm;
        assert_eq!(newest - profile[0].altitude_dm, 199 * 20);
        assert_eq!(profile[PROFILE_LEN - 1].grade_tenths, 100);
    }

    #[test]
    fn categories_follow_the_climb_score() {
        for (gain_dm, category) in [
            (200, Category::Uncategorised),
            (799, Category::Uncategorised),
            (800, Category::Four),
            (1_600, Category::Three),
            (3_199, Category::Three),
            (3_200, Category::Two),
            (4_800, Category::One),
            (6_400, Category::Hors),
            (18_000, Category::Hors),
        ] {
            assert_eq!(Category::of(gain_dm), category, "{gain_dm}");
        }
        assert_eq!(Category::Hors.name(), b"HC");
    }

    #[test]
    fn a_climb_knows_where_it_starts() {
        let mut climbs = Climbs::default();
        let mut at = 0;
        let events = ride(&mut climbs, &mut at, 4000, 5000, hill(6));
        let Some((_, Event::Finished(summary))) = events.last() else {
            panic!("{events:?}");
        };
        // The hill starts 1 km in.
        assert!(
            (980_000..=1_020_000).contains(&summary.start_mm),
            "{summary:?}"
        );
        assert_eq!(summary.category(), Category::Four);
    }

    #[test]
    fn interpolation_stays_on_the_line() {
        let a = Point {
            altitude_dm: 100,
            distance_mm: 0,
            time_ms: 0,
        };
        let b = Point {
            altitude_dm: 200,
            distance_mm: 40_000,
            time_ms: 4000,
        };
        let mid = a.toward(b, 20_000);
        assert_eq!((mid.altitude_dm, mid.time_ms), (150, 2000));
        assert_eq!(a.toward(a, 0), a);
        assert_eq!(grade_tenths(a, b), Some(250));
        assert_eq!(grade_tenths(a, a), None);
    }
}
