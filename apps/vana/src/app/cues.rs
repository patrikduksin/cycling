//! VANA's sound palette: a few short cues built from three notes, so each
//! event sounds like the others in its family and unlike the rest.
//!
//! The buzzer plays fixed patterns. VANA takes the short ones as a ladder of
//! low, middle and high notes by pitch, and the longest one that fits the
//! alert budget as the alarm. Rising runs mean go (start, resume, saved),
//! falling ones mean stop (pause), one note marks a lap and two low notes
//! ask for the charger. The rider can mute cues in Settings; a safety alarm
//! (a vehicle closing from behind) always sounds.
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
    LowBattery,
    /// One of a workout step's last seconds.
    Countdown,
    /// A workout step began, the workout ended or a test measured FTP.
    Step,
    /// A vehicle closing from behind.
    RadarApproach,
    /// The vehicles behind have passed.
    RadarClear,
    /// A ride alert: heart rate, power or cadence left the rider's band.
    BandAlert,
    /// Time to eat and drink.
    Fuel,
    /// Off the track being followed back.
    OffTrack,
    /// Back on it.
    OnTrack,
    /// Back at the start, following the track.
    AtStart,
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
            Self::LowBattery => &[Low, Low],
            Self::Countdown => &[Low],
            Self::Step => &[High, High],
            Self::RadarApproach => &[Alarm],
            // Low to high: the road behind is clear again.
            Self::RadarClear => &[Low, High],
            // Two high notes ask for attention without sounding like an alarm.
            Self::BandAlert => &[High, High],
            // A gentle middle pair, unlike the single lap note.
            Self::Fuel => &[Middle, Middle],
            Self::OffTrack => &[High, Low],
            Self::OnTrack => &[Low, Middle],
            Self::AtStart => &[Low, Middle, High, High],
        }
    }

    /// Sounds even when the rider muted cues, and is never replaced by one.
    pub const fn safety(self) -> bool {
        matches!(self, Self::RadarApproach)
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

/// The cue waiting to start and the run in progress.
#[derive(Default)]
pub(super) struct Cues {
    pending: Option<(Cue, u64)>,
    playing: Option<Cue>,
    run: Sequence,
}

impl Cues {
    /// Sound `cue` soon. A later cue replaces an earlier one, except that
    /// nothing replaces a safety alarm. `muted` keeps all but safety alarms
    /// quiet.
    pub(super) fn raise(&mut self, cue: Cue, muted: bool, now: u64) {
        if muted && !cue.safety() {
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
    pub(super) fn play(&mut self, sound: &mut impl Sound, now: u64) {
        if let Some((cue, at)) = self.pending.take()
            && now.saturating_sub(at) < STALE_MS
        {
            self.run.start(&resolve(sound.patterns(), cue), now);
            self.playing = Some(cue);
        }
        self.run.pump(sound, now);
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
        assert_eq!(ids(Cue::LowBattery), [21, 21]);
        assert_eq!(ids(Cue::RadarApproach), [22]);
        assert_eq!(ids(Cue::RadarClear), [21, 10]);
        assert_eq!(ids(Cue::BandAlert), [10, 10]);
        assert_eq!(ids(Cue::Fuel), [0, 0]);
        assert_eq!(ids(Cue::StillPaused), [0, 0, 10]);
        // One pattern stands for every note; none leaves cues silent.
        let single = [pattern(4, 3000, 100)];
        assert_eq!(resolve(&single, Cue::Saved).to_vec(), [4, 4, 4]);
        assert!(resolve(&[], Cue::Start).is_empty());
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
}
