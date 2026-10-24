//! Structured workouts: steps with a length and a target, grouped into
//! repeated blocks, built at compile time for the library or decoded from an
//! upload.
//!
//! A workout is a short list of distinct steps and the blocks that repeat
//! some of them, such as warm up, 5 × (3 min at 105 % / 3 min at 55 %), cool
//! down. Targets stay relative (% FTP, heart rate zone, % LTHR) until they
//! are drawn or checked, so a new FTP changes every workout at once.
use crate::ride::zones::TrainingProfile;

/// Distinct steps in one workout, before repeats.
pub const MAX_STEPS: usize = 12;
/// Repeated blocks in one workout.
pub const MAX_BLOCKS: usize = 8;
/// Steps a played workout can run through, repeats expanded.
pub const MAX_PLAYED: usize = 96;
/// Bytes in a workout or step name.
pub const NAME_BYTES: usize = 20;

/// Upper-case ASCII text of up to `NAME_BYTES`, buildable in `const`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; NAME_BYTES],
    len: u8,
}

impl core::fmt::Debug for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl Name {
    pub const EMPTY: Self = Self {
        bytes: [0; NAME_BYTES],
        len: 0,
    };

    /// `text` cut to `NAME_BYTES`, upper-cased; bytes the screen font cannot
    /// draw become spaces.
    pub const fn new(text: &str) -> Self {
        Self::from_bytes(text.as_bytes())
    }

    pub const fn from_bytes(text: &[u8]) -> Self {
        let mut bytes = [0; NAME_BYTES];
        let mut i = 0;
        while i < text.len() && i < NAME_BYTES {
            let byte = text[i].to_ascii_uppercase();
            bytes[i] = if byte >= b' ' && byte <= b'~' {
                byte
            } else {
                b' '
            };
            i += 1;
        }
        Self {
            bytes,
            len: i as u8,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// What a step is for; it names the step and colours nothing by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intensity {
    Warmup,
    Work,
    Recovery,
    Cooldown,
    /// The measured effort of an FTP test.
    Test,
}

impl Intensity {
    pub const ALL: [Self; 5] = [
        Self::Warmup,
        Self::Work,
        Self::Recovery,
        Self::Cooldown,
        Self::Test,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Warmup => "WARM UP",
            Self::Work => "WORK",
            Self::Recovery => "RECOVER",
            Self::Cooldown => "COOL DOWN",
            Self::Test => "TEST",
        }
    }
}

/// How long a step lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// Recorded (active) seconds; a pause adds none.
    Time(u16),
    /// Metres of wheel distance.
    Distance(u32),
    /// Until the rider marks a lap, or, for a ramp, until they cannot hold it.
    Open,
}

/// What the rider aims for during a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Ride as you feel; nothing is checked.
    Free,
    /// Power as a percentage range of FTP.
    Ftp { low: u8, high: u8 },
    /// Absolute power.
    Watts { low: u16, high: u16 },
    /// A heart-rate zone, 1-5.
    HeartZone(u8),
    /// Heart rate as a percentage range of LTHR.
    Lthr { low: u8, high: u8 },
    /// Cadence in rpm.
    Cadence { low: u8, high: u8 },
    /// An FTP ramp: `start` watts for the first `every` seconds, then `step`
    /// watts more each `every` seconds until the rider fails.
    Ramp { start: u16, step: u16, every: u16 },
}

/// What a target measures, once resolved against the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Power,
    Heart,
    Cadence,
}

/// A target as numbers: the band the rider aims for, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub metric: Metric,
    pub low: u16,
    pub high: u16,
}

impl Target {
    /// The band this target asks for `elapsed_s` into its step; `None` for a
    /// free ride.
    pub fn band(self, profile: &TrainingProfile, elapsed_s: u32) -> Option<Band> {
        let pct = |base: u16, pct: u8| ((u32::from(base) * u32::from(pct) + 50) / 100) as u16;
        let power = |low, high| Band {
            metric: Metric::Power,
            low,
            high,
        };
        let heart = |low, high| Band {
            metric: Metric::Heart,
            low,
            high,
        };
        Some(match self {
            Self::Free => return None,
            Self::Ftp { low, high } => {
                power(pct(profile.ftp_watts, low), pct(profile.ftp_watts, high))
            }
            Self::Watts { low, high } => power(low, high),
            Self::HeartZone(zone) => {
                let (low, next) = profile.heart_range(zone);
                heart(low as u16, (next.saturating_sub(1)) as u16)
            }
            Self::Lthr { low, high } => {
                heart(pct(profile.lthr_bpm, low), pct(profile.lthr_bpm, high))
            }
            Self::Cadence { low, high } => Band {
                metric: Metric::Cadence,
                low: u16::from(low),
                high: u16::from(high),
            },
            Self::Ramp { .. } => {
                let watts = self.ramp_watts(elapsed_s)?;
                power(watts, watts.saturating_add(watts / 20))
            }
        })
    }

    /// The ramp's watts `elapsed_s` into it.
    pub fn ramp_watts(self, elapsed_s: u32) -> Option<u16> {
        let Self::Ramp { start, step, every } = self else {
            return None;
        };
        let stage = elapsed_s / u32::from(every.max(1));
        Some(
            (u64::from(start) + u64::from(stage) * u64::from(step)).min(u64::from(u16::MAX)) as u16,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub intensity: Intensity,
    pub length: Length,
    pub target: Target,
    /// A short cue, such as "HIGH CADENCE"; may be empty.
    pub note: Name,
}

impl Step {
    pub const fn new(intensity: Intensity, length: Length, target: Target) -> Self {
        Self {
            intensity,
            length,
            target,
            note: Name::EMPTY,
        }
    }

    pub const fn note(mut self, note: &str) -> Self {
        self.note = Name::new(note);
        self
    }
}

/// `count` runs of the `len` steps from `first`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub first: u8,
    pub len: u8,
    pub count: u8,
}

/// FTP tests the player measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    /// FTP is 95 % of the average power of the whole `Intensity::Test` step.
    Twenty,
    /// FTP is 75 % of the best minute of the ramp step.
    Ramp,
}

impl Test {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Twenty => "20 MIN TEST",
            Self::Ramp => "RAMP TEST",
        }
    }
}

/// Why a workout cannot be played.
/// The shortest distance step: one update never crosses many of them.
pub const MIN_DISTANCE_M: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    NoSteps,
    TooManySteps,
    TooManyBlocks,
    /// A block reaches past the steps, overlaps another or repeats nothing.
    Block,
    /// A step that can never end or a target out of range.
    Step,
    /// Repeats expand past `MAX_PLAYED`.
    TooLong,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Workout {
    pub name: Name,
    steps: [Step; MAX_STEPS],
    step_count: u8,
    blocks: [Block; MAX_BLOCKS],
    block_count: u8,
    pub test: Option<Test>,
}

const NO_STEP: Step = Step::new(Intensity::Work, Length::Open, Target::Free);
const NO_BLOCK: Block = Block {
    first: 0,
    len: 0,
    count: 0,
};

impl Workout {
    /// A workout of `steps`, with `blocks` repeating some of them. Built-in
    /// workouts are made in `const` and checked by tests; uploads are
    /// checked by `validate` before use.
    pub const fn new(name: &str, steps: &[Step], blocks: &[Block], test: Option<Test>) -> Self {
        let mut workout = Self {
            name: Name::new(name),
            steps: [NO_STEP; MAX_STEPS],
            step_count: 0,
            blocks: [NO_BLOCK; MAX_BLOCKS],
            block_count: 0,
            test,
        };
        let mut i = 0;
        while i < steps.len() && i < MAX_STEPS {
            workout.steps[i] = steps[i];
            i += 1;
        }
        workout.step_count = i as u8;
        let mut i = 0;
        while i < blocks.len() && i < MAX_BLOCKS {
            workout.blocks[i] = blocks[i];
            i += 1;
        }
        workout.block_count = i as u8;
        workout
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps[..usize::from(self.step_count)]
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks[..usize::from(self.block_count)]
    }

    /// Check what `new` cannot: limits, block bounds, step lengths and targets.
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.step_count == 0 {
            return Err(Invalid::NoSteps);
        }
        let mut previous_end = 0;
        let mut blocks: heapless::Vec<Block, MAX_BLOCKS> = self.blocks().iter().copied().collect();
        blocks.sort_unstable_by_key(|block| block.first);
        for block in blocks.iter() {
            let end = usize::from(block.first) + usize::from(block.len);
            if block.len == 0
                || block.count == 0
                || end > self.steps().len()
                || usize::from(block.first) < previous_end
            {
                return Err(Invalid::Block);
            }
            previous_end = end;
        }
        for step in self.steps() {
            let length_ok = match step.length {
                Length::Time(secs) => secs > 0,
                Length::Distance(metres) => metres >= MIN_DISTANCE_M,
                Length::Open => true,
            };
            let step_open = matches!(step.length, Length::Open);
            let target_ok = match step.target {
                Target::Free => true,
                Target::Ftp { low, high } | Target::Lthr { low, high } => {
                    low > 0 && low <= high && high <= 250
                }
                Target::Watts { low, high } => low > 0 && low <= high && high <= 2000,
                Target::HeartZone(zone) => (1..=5).contains(&zone),
                Target::Cadence { low, high } => low > 0 && low <= high,
                Target::Ramp {
                    start,
                    step: rise,
                    every,
                } => start > 0 && rise > 0 && every > 0 && step_open,
            };
            if !length_ok || !target_ok {
                return Err(Invalid::Step);
            }
        }
        if self.played_len() > MAX_PLAYED {
            return Err(Invalid::TooLong);
        }
        Ok(())
    }

    /// Steps played, repeats expanded.
    pub fn played_len(&self) -> usize {
        let mut len = self.steps().len();
        for block in self.blocks() {
            len += usize::from(block.len) * usize::from(block.count.saturating_sub(1));
        }
        len
    }

    /// The played steps in order: (step index, repetition, of how many).
    pub fn played(&self) -> impl Iterator<Item = Played> + '_ {
        let mut index = 0usize;
        let mut repetition = 0u8;
        core::iter::from_fn(move || {
            let step = *self.steps().get(index)?;
            let block = self.block_at(index);
            let played = Played {
                index: index as u8,
                repetition,
                of: block.map_or(1, |b| b.count),
                step,
            };
            match block {
                Some(block) if index + 1 == usize::from(block.first + block.len) => {
                    if repetition + 1 < block.count {
                        repetition += 1;
                        index = usize::from(block.first);
                    } else {
                        repetition = 0;
                        index += 1;
                    }
                }
                _ => index += 1,
            }
            Some(played)
        })
    }

    fn block_at(&self, index: usize) -> Option<Block> {
        self.blocks().iter().copied().find(|block| {
            (usize::from(block.first)..usize::from(block.first + block.len)).contains(&index)
        })
    }

    /// Planned recorded seconds of the timed steps, repeats included; distance
    /// and open steps count nothing.
    pub fn planned_secs(&self) -> u32 {
        self.played()
            .map(|played| match played.step.length {
                Length::Time(secs) => u32::from(secs),
                _ => 0,
            })
            .sum()
    }

    /// Some step ends only when the rider says so or by distance.
    pub fn has_untimed_steps(&self) -> bool {
        self.steps()
            .iter()
            .any(|step| !matches!(step.length, Length::Time(_)))
    }
}

/// One step as it is played: which step, and which run of its block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Played {
    pub index: u8,
    /// Zero-based run of the block the step is in.
    pub repetition: u8,
    /// Runs of that block; one outside blocks.
    pub of: u8,
    pub step: Step,
}

#[cfg(test)]
mod tests {
    use super::*;
    use Intensity::*;

    const fn ftp(low: u8, high: u8) -> Target {
        Target::Ftp { low, high }
    }

    fn intervals() -> Workout {
        Workout::new(
            "VO2 5X3",
            &[
                Step::new(Warmup, Length::Time(600), ftp(50, 65)),
                Step::new(Work, Length::Time(180), ftp(105, 115)),
                Step::new(Recovery, Length::Time(180), ftp(45, 55)),
                Step::new(Cooldown, Length::Time(300), ftp(40, 50)),
            ],
            &[Block {
                first: 1,
                len: 2,
                count: 5,
            }],
            None,
        )
    }

    #[test]
    fn repeats_expand_in_order() {
        let workout = intervals();
        assert_eq!(workout.validate(), Ok(()));
        let order: std::vec::Vec<_> = workout
            .played()
            .map(|p| (p.index, p.repetition, p.of))
            .collect();
        let mut want = std::vec![(0, 0, 1)];
        for rep in 0..5 {
            want.push((1, rep, 5));
            want.push((2, rep, 5));
        }
        want.push((3, 0, 1));
        assert_eq!(order, want);
        assert_eq!(workout.played_len(), 12);
        assert_eq!(workout.planned_secs(), 600 + 5 * 360 + 300);
    }

    #[test]
    fn targets_resolve_against_the_profile() {
        let profile = TrainingProfile::new(250, 170).unwrap();
        let band = ftp(88, 94).band(&profile, 0).unwrap();
        assert_eq!(
            (band.metric, band.low, band.high),
            (Metric::Power, 220, 235)
        );
        let heart = Target::HeartZone(2).band(&profile, 0).unwrap();
        assert_eq!(heart.metric, Metric::Heart);
        assert_eq!(profile.heart_zone(heart.low), 2);
        assert_eq!(profile.heart_zone(heart.high), 2);
        assert_eq!(profile.heart_zone(heart.high + 1), 3);
        assert_eq!(Target::Free.band(&profile, 0), None);
        let ramp = Target::Ramp {
            start: 100,
            step: 20,
            every: 60,
        };
        assert_eq!(ramp.ramp_watts(0), Some(100));
        assert_eq!(ramp.ramp_watts(59), Some(100));
        assert_eq!(ramp.ramp_watts(60), Some(120));
        assert_eq!(ramp.band(&profile, 600).unwrap().low, 300);
    }

    #[test]
    fn a_steep_uploaded_ramp_tops_out_instead_of_overflowing() {
        let ramp = Target::Ramp {
            start: 1,
            step: u16::MAX,
            every: 1,
        };
        assert_eq!(ramp.ramp_watts(65_537), Some(u16::MAX));
        assert_eq!(ramp.ramp_watts(u32::MAX), Some(u16::MAX));
    }

    #[test]
    fn validation_rejects_impossible_workouts() {
        let empty = Workout::new("EMPTY", &[], &[], None);
        assert_eq!(empty.validate(), Err(Invalid::NoSteps));
        let step = Step::new(Work, Length::Time(60), ftp(90, 95));
        let past = Workout::new(
            "PAST",
            &[step],
            &[Block {
                first: 0,
                len: 2,
                count: 2,
            }],
            None,
        );
        assert_eq!(past.validate(), Err(Invalid::Block));
        let overlapping = Workout::new(
            "OVERLAP",
            &[step, step, step],
            &[
                Block {
                    first: 0,
                    len: 2,
                    count: 2,
                },
                Block {
                    first: 1,
                    len: 2,
                    count: 2,
                },
            ],
            None,
        );
        assert_eq!(overlapping.validate(), Err(Invalid::Block));
        let zero = Workout::new(
            "ZERO",
            &[Step::new(Work, Length::Time(0), Target::Free)],
            &[],
            None,
        );
        assert_eq!(zero.validate(), Err(Invalid::Step));
        let inverted = Workout::new(
            "INV",
            &[Step::new(Work, Length::Time(5), ftp(90, 80))],
            &[],
            None,
        );
        assert_eq!(inverted.validate(), Err(Invalid::Step));
        let timed_ramp = Workout::new(
            "RAMP",
            &[Step::new(
                Test,
                Length::Time(60),
                Target::Ramp {
                    start: 100,
                    step: 20,
                    every: 60,
                },
            )],
            &[],
            None,
        );
        assert_eq!(timed_ramp.validate(), Err(Invalid::Step));
        let long = Workout::new(
            "LONG",
            &[step, step],
            &[Block {
                first: 0,
                len: 2,
                count: 50,
            }],
            None,
        );
        assert_eq!(long.validate(), Err(Invalid::TooLong));
    }

    #[test]
    fn names_are_upper_case_and_bounded() {
        let name = Name::new("Sweet spot 2x20 with a very long name");
        assert_eq!(name.as_str(), "SWEET SPOT 2X20 WITH");
        assert_eq!(Name::new("tab\there").as_str(), "TAB HERE");
    }
}
