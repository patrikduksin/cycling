//! VANA's sound palette: a few short cues built from three notes, so each
//! event sounds like the others in its family and unlike the rest.
//!
//! The buzzer plays fixed patterns. VANA takes the short ones as a ladder of
//! low, middle and high notes by pitch, and the longest one that fits the
//! alert budget as the alarm. Rising runs mean go (start, resume, saved),
//! falling ones mean stop (pause), one note marks a lap and two low notes
//! ask for the charger. A vehicle closing from behind sounds the alarm, then a
//! low, middle or high note by how near it is. The rider can mute cues in
//! Settings; the radar cues are safety cues and always sound.
use crate::ride::course::nav::Navigation;
use device_api::sound::{Pattern, Sound};
use firmware_services::sound::Sequence;

/// The longest pattern used as the alarm.
const ALARM_MAX_MS: u16 = 300;
/// A note longer than this is too long for a cue.
const NOTE_MAX_MS: u16 = 150;
/// A cue not started this long after its event is dropped.
const STALE_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    Start,
    Pause,
    Resume,
    Lap,
    Saved,
    /// Riding on while the ride is still paused.
    StillPaused,
    /// The saved ride set a personal best.
    Best,
    LowBattery,
    /// One of a workout step's last seconds.
    Countdown,
    /// A workout step began, the workout ended or a test measured FTP.
    Step,
    /// A vehicle appeared behind.
    RadarApproach,
    /// The nearest vehicle is near.
    RadarNear,
    /// The nearest vehicle is close.
    RadarClose,
    /// The nearest vehicle is passing.
    RadarPassing,
    /// The vehicles behind have passed.
    RadarClear,
    /// A ride alert: heart rate, power or cadence left the rider's band.
    BandAlert,
    /// Time to eat and drink.
    Fuel,
    OffCourse,
    /// Back on it, or following it began.
    OnCourse,
    TurnSoon,
    TurnNow,
    /// The rider marked a moment for the trace.
    Mark,
}

#[derive(Clone, Copy)]
enum Note {
    Low,
    Middle,
    High,
    Alarm,
}

impl Cue {
    const fn notes(self) -> &'static [Note] {
        use Note::*;
        match self {
            Self::Start => &[Middle, High],
            Self::Resume => &[High],
            Self::Pause => &[High, Middle],
            Self::Lap => &[Middle],
            Self::Saved => &[Low, Middle, High],
            Self::StillPaused => &[Middle, Middle, High],
            // The saved run, topped with a held high note.
            Self::Best => &[Middle, High, High],
            Self::LowBattery => &[Low, Low],
            Self::Countdown => &[Low],
            Self::Step => &[High, High],
            Self::RadarApproach => &[Alarm],
            // The note climbs as the vehicle closes in.
            Self::RadarNear => &[Low],
            Self::RadarClose => &[Middle],
            Self::RadarPassing => &[High],
            // Low to high: the road behind is clear again.
            Self::RadarClear => &[Low, High],
            // Two high notes ask for attention without sounding like an alarm.
            Self::BandAlert => &[High, High],
            // A gentle middle pair, unlike the single lap note.
            Self::Fuel => &[Middle, Middle],
            Self::OffCourse => &[High, Low],
            Self::OnCourse => &[Low, High],
            Self::TurnSoon => &[Low, Middle],
            Self::TurnNow => &[High],
            Self::Mark => &[Middle, Middle, Middle],
        }
    }

    /// The cue's name in a trace: `start`, `radar_clear`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Lap => "lap",
            Self::Saved => "saved",
            Self::StillPaused => "still_paused",
            Self::Best => "best",
            Self::LowBattery => "low_battery",
            Self::Countdown => "countdown",
            Self::Step => "step",
            Self::RadarApproach => "radar_approach",
            Self::RadarNear => "radar_near",
            Self::RadarClose => "radar_close",
            Self::RadarPassing => "radar_passing",
            Self::RadarClear => "radar_clear",
            Self::BandAlert => "band_alert",
            Self::Fuel => "fuel",
            Self::OffCourse => "off_course",
            Self::OnCourse => "on_course",
            Self::TurnSoon => "turn_soon",
            Self::TurnNow => "turn_now",
            Self::Mark => "mark",
        }
    }

    /// Sounds even when the rider muted cues, and is never replaced by one.
    pub const fn safety(self) -> bool {
        matches!(
            self,
            Self::RadarApproach | Self::RadarNear | Self::RadarClose | Self::RadarPassing
        )
    }

    const fn navigation(self) -> Option<Navigation> {
        match self {
            Self::OffCourse => Some(Navigation::OffCourse),
            Self::OnCourse => Some(Navigation::OnCourse),
            Self::TurnNow => Some(Navigation::TurnNow),
            Self::TurnSoon => Some(Navigation::TurnSoon),
            _ => None,
        }
    }
}

impl From<Navigation> for Cue {
    fn from(cue: Navigation) -> Self {
        match cue {
            Navigation::OffCourse => Self::OffCourse,
            Navigation::OnCourse => Self::OnCourse,
            Navigation::TurnNow => Self::TurnNow,
            Navigation::TurnSoon => Self::TurnSoon,
        }
    }
}

/// The pattern ids a cue plays on this sound, in order. A sound with fewer
/// notes plays what it has: one short pattern stands for every note.
fn resolve(patterns: &[Pattern], cue: Cue) -> heapless::Vec<u8, 4> {
    let mut short: heapless::Vec<Pattern, 8> = patterns
        .iter()
        .filter(|pattern| pattern.nominal_ms <= NOTE_MAX_MS)
        .take(8)
        .copied()
        .collect();
    short.sort_unstable_by_key(|pattern| pattern.frequency_hz);
    let alarm = patterns
        .iter()
        .filter(|pattern| pattern.nominal_ms <= ALARM_MAX_MS)
        .max_by_key(|pattern| pattern.nominal_ms);
    cue.notes()
        .iter()
        .filter_map(|note| {
            let pattern = match note {
                Note::Low => short.first(),
                Note::Middle => short.get(short.len() / 2),
                Note::High => short.last(),
                Note::Alarm => alarm,
            };
            pattern.map(|pattern| pattern.id)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Played {
    pub cue: Cue,
    pub pattern: u8,
    pub note: u8,
    pub of: u8,
}

/// Navigation cues waiting for the buzzer, one of each kind.
#[derive(Default)]
struct Waiting([bool; 4]);

impl Waiting {
    fn add(&mut self, cue: Navigation) {
        self.0[cue as usize] = true;
    }

    fn any(&self) -> bool {
        self.0.contains(&true)
    }

    /// The most urgent cue waiting that still `holds`; those before it that
    /// no longer do are dropped.
    fn next(&mut self, holds: impl Fn(Navigation) -> bool) -> Option<Navigation> {
        Navigation::BY_URGENCY
            .into_iter()
            .find(|&cue| core::mem::take(&mut self.0[cue as usize]) && holds(cue))
    }
}

#[derive(Default)]
pub(super) struct Cues {
    pending: Option<(Cue, u64)>,
    navigation_waiting: Waiting,
    playing: Option<Cue>,
    run: Sequence,
    note: u8,
    of: u8,
    played: Option<Played>,
}

impl Cues {
    /// Sound `cue` soon. A later cue replaces an earlier one, except that
    /// only another alarm replaces a safety alarm, and a navigation cue
    /// waits apart, beside one of each other kind, until no other cue is
    /// waiting or sounding. `muted` keeps all but safety alarms quiet.
    pub(super) fn raise(&mut self, cue: Cue, muted: bool, now: u64) {
        if muted && !cue.safety() {
            return;
        }
        if let Some(cue) = cue.navigation() {
            self.navigation_waiting.add(cue);
            return;
        }
        let busy = |held: Option<Cue>| held.is_some_and(Cue::safety);
        let alarm_held =
            busy(self.pending.map(|(cue, _)| cue)) || (self.run.is_playing() && busy(self.playing));
        if alarm_held && !cue.safety() {
            return;
        }
        self.pending = Some((cue, now));
    }

    /// Start a raised cue and offer its notes to `sound`. Call once per loop.
    /// Navigation cues waiting play most urgent first, each dropped unless
    /// it `holds` still, until the buzzer takes its first note. One cut off
    /// before its last note waits again.
    pub(super) fn play(
        &mut self,
        sound: &mut impl Sound,
        now: u64,
        holds: impl Fn(Navigation) -> bool,
    ) {
        let next = match self.pending.take() {
            Some((cue, at)) => (now.saturating_sub(at) < STALE_MS).then_some(cue),
            None if !self.run.is_playing() => self.navigation_waiting.next(&holds).map(Cue::from),
            None => None,
        };
        if let Some(cue) = next {
            if let Some(cut) = self
                .playing
                .filter(|_| self.run.is_playing())
                .and_then(Cue::navigation)
            {
                self.navigation_waiting.add(cut);
            }
            let notes = resolve(sound.patterns(), cue);
            self.run.start(&notes, now);
            self.playing = Some(cue);
            self.note = 0;
            self.of = notes.len() as u8;
        }
        let unheard = self.note == 0 && self.run.is_playing();
        if let Some(cue) = self.playing.and_then(Cue::navigation)
            && unheard
            && !holds(cue)
        {
            self.run.clear();
            self.playing = None;
        }
        if let (Some(pattern), Some(cue)) = (self.run.pump(sound, now), self.playing) {
            self.note += 1;
            self.played = Some(Played {
                cue,
                pattern,
                note: self.note,
                of: self.of,
            });
        }
    }

    /// A cue waits for the buzzer or is still sounding.
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.navigation_waiting.any() || self.run.is_playing()
    }

    pub(super) fn take_played(&mut self) -> Option<Played> {
        self.played.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(id: u8, frequency_hz: u16, nominal_ms: u16) -> Pattern {
        Pattern {
            id,
            frequency_hz,
            nominal_ms,
            guard_ms: 500,
        }
    }

    #[test]
    fn cues_climb_the_short_patterns_by_pitch() {
        // The C606 buzzer's patterns.
        let c606 = [
            pattern(0, 3000, 75),
            pattern(10, 4000, 100),
            pattern(21, 2500, 150),
            pattern(22, 3000, 300),
        ];
        let ids = |cue| resolve(&c606, cue).to_vec();
        assert_eq!(ids(Cue::Start), [0, 10]);
        assert_eq!(ids(Cue::Pause), [10, 0]);
        assert_eq!(ids(Cue::Resume), [10]);
        assert_eq!(ids(Cue::Lap), [0]);
        assert_eq!(ids(Cue::Saved), [21, 0, 10]);
        assert_eq!(ids(Cue::Best), [0, 10, 10]);
        assert_eq!(ids(Cue::LowBattery), [21, 21]);
        assert_eq!(ids(Cue::RadarApproach), [22]);
        assert_eq!(ids(Cue::RadarNear), [21]);
        assert_eq!(ids(Cue::RadarClose), [0]);
        assert_eq!(ids(Cue::RadarPassing), [10]);
        assert_eq!(ids(Cue::RadarClear), [21, 10]);
        assert_eq!(ids(Cue::BandAlert), [10, 10]);
        assert_eq!(ids(Cue::Fuel), [0, 0]);
        assert_eq!(ids(Cue::StillPaused), [0, 0, 10]);
        assert_eq!(ids(Cue::OffCourse), [10, 21]);
        assert_eq!(ids(Cue::OnCourse), [21, 10]);
        assert_eq!(ids(Cue::TurnSoon), [21, 0]);
        assert_eq!(ids(Cue::TurnNow), [10]);
        assert_eq!(ids(Cue::Mark), [0, 0, 0]);
        // One pattern stands for every note; none leaves cues silent.
        let single = [pattern(4, 3000, 100)];
        assert_eq!(resolve(&single, Cue::Saved).to_vec(), [4, 4, 4]);
        assert!(resolve(&[], Cue::Start).is_empty());
    }

    struct Open;

    impl Sound for Open {
        fn patterns(&self) -> &'static [Pattern] {
            const C606: [Pattern; 3] = [
                Pattern {
                    id: 0,
                    frequency_hz: 3000,
                    nominal_ms: 75,
                    guard_ms: 500,
                },
                Pattern {
                    id: 10,
                    frequency_hz: 4000,
                    nominal_ms: 100,
                    guard_ms: 500,
                },
                Pattern {
                    id: 21,
                    frequency_hz: 2500,
                    nominal_ms: 150,
                    guard_ms: 500,
                },
            ];
            &C606
        }
        fn snapshot(&self) -> device_api::sound::Snapshot {
            unreachable!("cues never read the buzzer's state")
        }
        fn play(&mut self, _: u8, _: u64) -> Result<(), device_api::observation::Error> {
            Ok(())
        }
        fn stop(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
            Ok(())
        }
    }

    #[test]
    fn each_note_played_names_its_cue_and_its_place_in_it() {
        let mut cues = Cues::default();
        let mut heard = std::vec::Vec::new();
        cues.raise(Cue::Start, false, 0);
        for now in [0, 500, 1000] {
            cues.play(&mut Open, now, |_| true);
            heard.extend(cues.take_played());
        }
        cues.raise(Cue::Saved, false, 1000);
        cues.play(&mut Open, 1000, |_| true);
        heard.extend(cues.take_played());
        cues.raise(Cue::Lap, false, 1500);
        cues.play(&mut Open, 1500, |_| true);
        heard.extend(cues.take_played());
        let played = |cue, pattern, note, of| Played {
            cue,
            pattern,
            note,
            of,
        };
        assert_eq!(
            heard,
            [
                played(Cue::Start, 0, 1, 2),
                played(Cue::Start, 10, 2, 2),
                played(Cue::Saved, 21, 1, 3),
                played(Cue::Lap, 0, 1, 1),
            ]
        );
        assert_eq!(Cue::RadarClear.name(), "radar_clear");
    }

    #[test]
    fn muted_cues_stay_quiet_but_alarms_sound_and_win() {
        let mut cues = Cues::default();
        cues.raise(Cue::Lap, true, 0);
        assert_eq!(cues.pending, None);
        cues.raise(Cue::RadarApproach, true, 0);
        cues.raise(Cue::Lap, false, 10);
        assert_eq!(cues.pending, Some((Cue::RadarApproach, 0)));
        let mut later = Cues::default();
        later.raise(Cue::Lap, false, 0);
        later.raise(Cue::Saved, false, 10);
        assert_eq!(later.pending, Some((Cue::Saved, 10)));
    }

    fn heard(
        cues: &mut Cues,
        until: u64,
        holds: impl Fn(Navigation) -> bool,
    ) -> std::vec::Vec<Cue> {
        let mut heard = std::vec::Vec::new();
        for now in (0..until).step_by(50) {
            cues.play(&mut Open, now, &holds);
            heard.extend(cues.take_played().filter(|p| p.note == 1).map(|p| p.cue));
        }
        heard
    }

    #[test]
    fn a_navigation_cue_under_a_radar_alarm_plays_once_the_alarm_ends() {
        let mut cues = Cues::default();
        cues.raise(Cue::RadarApproach, false, 0);
        cues.raise(Cue::TurnNow, false, 0);
        cues.raise(Cue::RadarPassing, false, 0);
        assert_eq!(
            heard(&mut cues, 5_000, |_| true),
            [Cue::RadarPassing, Cue::TurnNow]
        );
        let mut later = Cues::default();
        later.raise(Cue::OffCourse, false, 0);
        later.raise(Cue::RadarClear, false, 0);
        assert_eq!(
            heard(&mut later, 5_000, |_| true),
            [Cue::RadarClear, Cue::OffCourse]
        );
    }

    #[test]
    fn a_held_back_navigation_cue_that_no_longer_holds_is_dropped() {
        let mut cues = Cues::default();
        cues.raise(Cue::RadarApproach, false, 0);
        cues.raise(Cue::TurnNow, false, 0);
        assert_eq!(
            heard(&mut cues, 5_000, |cue| cue != Navigation::TurnNow),
            [Cue::RadarApproach]
        );
        let mut muted = Cues::default();
        muted.raise(Cue::TurnNow, true, 0);
        assert!(heard(&mut muted, 1_000, |_| true).is_empty());
    }

    struct GuardedBuzzer {
        free_at_ms: u64,
    }

    impl Sound for GuardedBuzzer {
        fn patterns(&self) -> &'static [Pattern] {
            Open.patterns()
        }
        fn snapshot(&self) -> device_api::sound::Snapshot {
            unreachable!("cues never read the buzzer's state")
        }
        fn play(&mut self, id: u8, now: u64) -> Result<(), device_api::observation::Error> {
            if now < self.free_at_ms {
                return Err(device_api::observation::Error::Unavailable);
            }
            let guard = self
                .patterns()
                .iter()
                .find(|p| p.id == id)
                .map_or(0, |p| p.guard_ms);
            self.free_at_ms = now + u64::from(guard);
            Ok(())
        }
        fn stop(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
            Ok(())
        }
    }

    #[test]
    fn a_navigation_cue_cut_off_by_the_next_radar_beep_plays_after_it() {
        let (mut cues, mut buzzer) = (Cues::default(), GuardedBuzzer { free_at_ms: 0 });
        let mut heard = std::vec::Vec::new();
        for now in (0..4_000).step_by(50) {
            if now % 400 == 0 && now < 1_200 {
                cues.raise(Cue::RadarPassing, false, now);
            }
            if now == 100 {
                cues.raise(Cue::OffCourse, false, now);
            }
            cues.play(&mut buzzer, now, |_| true);
            heard.extend(cues.take_played().filter(|p| p.note == 1).map(|p| p.cue));
        }
        assert_eq!(heard.last(), Some(&Cue::OffCourse), "{heard:?}");
    }

    #[test]
    fn a_turn_now_whose_turn_passes_before_the_buzzer_takes_it_never_sounds() {
        let (mut cues, mut buzzer) = (Cues::default(), GuardedBuzzer { free_at_ms: 0 });
        let mut heard = std::vec::Vec::new();
        for now in (0..2_000).step_by(50) {
            if now == 0 {
                cues.raise(Cue::RadarPassing, false, now);
            }
            if now == 50 {
                cues.raise(Cue::TurnNow, false, now);
            }
            cues.play(&mut buzzer, now, |_| now < 100);
            heard.extend(cues.take_played().filter(|p| p.note == 1).map(|p| p.cue));
        }
        assert_eq!(heard, [Cue::RadarPassing]);
    }

    #[test]
    fn navigation_cues_waiting_together_each_play_off_course_first() {
        let mut cues = Cues::default();
        cues.raise(Cue::RadarClear, false, 0);
        cues.raise(Cue::OnCourse, false, 0);
        cues.raise(Cue::TurnSoon, false, 0);
        assert_eq!(
            heard(&mut cues, 8_000, |_| true),
            [Cue::RadarClear, Cue::OnCourse, Cue::TurnSoon]
        );
        let mut more = Cues::default();
        more.raise(Cue::RadarApproach, false, 0);
        for cue in [Cue::TurnSoon, Cue::TurnNow, Cue::OnCourse, Cue::OffCourse] {
            more.raise(cue, false, 0);
        }
        assert_eq!(
            heard(&mut more, 8_000, |_| true),
            [
                Cue::RadarApproach,
                Cue::OffCourse,
                Cue::OnCourse,
                Cue::TurnNow,
                Cue::TurnSoon
            ]
        );
    }

    struct Course {
        nav: crate::ride::course::nav::Navigator,
        cues: Cues,
        buzzer: GuardedBuzzer,
        heard: std::vec::Vec<(u64, Cue)>,
    }

    impl Course {
        fn joined(buzzer_free_at_ms: u64) -> Self {
            let mut course = Self {
                nav: Default::default(),
                cues: Cues::default(),
                buzzer: GuardedBuzzer {
                    free_at_ms: buzzer_free_at_ms,
                },
                heard: std::vec::Vec::new(),
            };
            let fix = Self::fix(0, 0.0, false);
            course
                .nav
                .step(Observed::Joined(fix), &[], |_| (0.0, 100.0));
            course
        }

        fn fix(now: u64, off_m: f32, paused: bool) -> crate::ride::course::nav::MatchedFix {
            crate::ride::course::nav::MatchedFix {
                taken_loop_ms: now,
                along_mm: 100_000,
                to_end_mm: 10_000_000,
                off_m,
                here: (off_m, 100.0),
                forward: true,
                speed_mm_s: Some(4_000),
                paused,
            }
        }

        fn ride(&mut self, from_ms: u64, to_ms: u64, off_m: f32, paused: bool) {
            for now in (from_ms..to_ms).step_by(50) {
                if (now - from_ms).is_multiple_of(1_000) {
                    let fix = Observed::Fix(Self::fix(now, off_m, paused));
                    let event = self.nav.step(fix, &[], |_| (0.0, 100.0));
                    if let Some(Event::OffCourse | Event::StillOff) = event {
                        self.cues.raise(Cue::OffCourse, false, now);
                    }
                }
                let nav = &self.nav;
                self.cues
                    .play(&mut self.buzzer, now, |cue| nav.holds(cue, &[]));
                let first = self.cues.take_played().filter(|p| p.note == 1);
                self.heard.extend(first.map(|p| (now, p.cue)));
            }
        }
    }

    use crate::ride::course::nav::{Event, Observed};

    #[test]
    fn an_off_course_waiting_for_the_buzzer_when_the_ride_pauses_never_sounds() {
        let mut course = Course::joined(6_800);
        course.ride(1_000, 6_200, 44.0, false);
        course.ride(6_200, 60_000, 44.0, true);
        assert_eq!(course.heard, []);
    }

    #[test]
    fn off_course_sounds_again_once_the_paused_ride_resumes() {
        let mut course = Course::joined(6_800);
        course.ride(1_000, 6_200, 44.0, false);
        course.ride(6_200, 10_000, 44.0, true);
        course.ride(10_000, 30_000, 44.0, false);
        assert_eq!(course.heard, [(21_000, Cue::OffCourse)], "reminded");
        course.ride(30_000, 32_000, 0.0, false);
        course.ride(32_000, 40_000, 44.0, false);
        assert_eq!(course.heard[1..], [(37_000, Cue::OffCourse)], "left again");
    }
}
