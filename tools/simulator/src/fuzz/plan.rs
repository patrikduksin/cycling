use crate::world::Side;
use device_api::input::Button;
use std::ops::RangeInclusive;

/// The seeds `cycling fuzz --fixed` runs: synthetic courses only, so every
/// machine runs the same sessions.
pub const FIXED: RangeInclusive<u64> = 1..=200;

/// SplitMix64: tiny, and every u64 seed is a good stream. Integers only, so a
/// seed draws the same plan on every platform.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; zero when `n` is.
    pub fn below(&mut self, n: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }

    pub fn between(&mut self, range: RangeInclusive<u64>) -> u64 {
        range.start() + self.below(range.end() - range.start() + 1)
    }

    pub fn percent(&mut self, p: u64) -> bool {
        self.below(100) < p
    }

    pub fn coin(&mut self) -> bool {
        self.percent(50)
    }

    /// Log-uniform by octaves in `range`: mostly short, sometimes long.
    pub fn octaves(&mut self, range: RangeInclusive<u64>) -> u64 {
        let (lo, hi) = (*range.start().max(&1), *range.end());
        let octaves = u64::from((hi / lo).ilog2()) + 1;
        let bottom = lo << self.below(octaves);
        self.between(bottom..=(bottom * 2).min(hi))
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub seed: u64,
    pub setup: Setup,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// Sensors in range and paired before any fault.
    pub sensors: Vec<Sensor>,
    pub card: Card,
    pub course: Course,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    Heart,
    Power,
    SpeedCadence,
    Radar,
}

impl Sensor {
    pub const ALL: [Sensor; 4] = [Self::Heart, Self::Power, Self::SpeedCadence, Self::Radar];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    None,
    /// Fitted and initialized, so rides record onto it.
    Ready,
    /// Fitted but not answering this boot.
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Course {
    /// `center` indexes `CENTERS`.
    Loop {
        center: u8,
        length_m: u32,
        climb_m: u16,
    },
    /// A straight road east through (distance m, elevation m) knots.
    Profile { center: u8, knots: Vec<(u32, i16)> },
    /// A track from the corpus, by index; an empty corpus rides
    /// `empty_corpus_loop(pick)` instead.
    Corpus { pick: u64 },
}

pub fn empty_corpus_loop(pick: u64) -> Course {
    Course::Loop {
        center: (pick % CENTERS.len() as u64) as u8,
        length_m: (2_000 + pick % 30_000) as u32,
        climb_m: (pick % 200) as u16,
    }
}

const MID_LATITUDE: (f64, f64) = (-33.45, -70.66);
const NEAR_POLE: (f64, f64) = (79.9, 11.9);
const ANTIMERIDIAN: (f64, f64) = (-16.8, 179.995);
const NULL_ISLAND: (f64, f64) = (0.0005, -0.0005);
pub const CENTERS: [(f64, f64); 4] = [MID_LATITUDE, NEAR_POLE, ANTIMERIDIAN, NULL_ISLAND];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RideAction {
    Start,
    Pause,
    Resume,
    Finish,
}

impl RideAction {
    pub fn command(self) -> &'static str {
        match self {
            Self::Start => "RIDE START",
            Self::Pause => "RIDE PAUSE",
            Self::Resume => "RIDE RESUME",
            Self::Finish => "RIDE FINISH",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait {
        ms: u32,
    },
    Press(Button),
    Hold(Button, u16),
    Ride {
        kmh: u8,
        watts: u16,
        bpm: u8,
        rpm: u8,
    },
    Stop,
    /// A ride action from the host console; a refusal is legal.
    Host(RideAction),
    Dropout {
        sensor: Sensor,
        present: bool,
    },
    GpsFix(bool),
    Barometer(bool),
    Vehicle {
        range_m: u16,
        closing_kmh: u8,
        fast: bool,
        side: Side,
    },
    Battery {
        percent: u8,
        charging: bool,
    },
    /// Power cut without warning; the device boots while the plan goes on.
    PowerCut,
    /// The internal flash tears its next mutation after `after` succeed.
    TornWrite {
        after: u8,
        partial: u16,
    },
    /// The card fails a write after `after` succeed, until the next boot.
    CardRefusal {
        after: u8,
        partial: u16,
    },
}

impl Step {
    /// The world time a step takes by itself.
    pub fn nominal_ms(self) -> u64 {
        match self {
            Self::Wait { ms } => u64::from(ms),
            Self::Press(_) => 400,
            Self::Hold(_, ms) => u64::from(ms) + 400,
            _ => 0,
        }
    }
}

const BUTTONS: [Button; 3] = [Button::TopLeft, Button::BottomLeft, Button::BottomRight];

type Draw = fn(&mut Rng, &Setup) -> Option<Step>;

const WEIGHTS: &[(u64, Draw)] = &[
    (30, |rng, _| Some(Step::Press(rng.pick(&BUTTONS)))),
    (20, |rng, _| {
        Some(Step::Wait {
            ms: rng.octaves(100..=600_000) as u32,
        })
    }),
    (5, |rng, _| {
        Some(Step::Hold(
            rng.pick(&BUTTONS),
            rng.octaves(200..=4_000) as u16,
        ))
    }),
    (10, |rng, _| Some(ride(rng))),
    (3, |_, _| Some(Step::Stop)),
    (8, |rng, _| {
        Some(Step::Host(rng.pick(&[
            RideAction::Start,
            RideAction::Pause,
            RideAction::Resume,
            RideAction::Finish,
        ])))
    }),
    (4, |rng, setup| {
        let sensor = setup
            .sensors
            .get(rng.below(setup.sensors.len() as u64) as usize)?;
        Some(Step::Dropout {
            sensor: *sensor,
            present: rng.percent(60),
        })
    }),
    (2, |rng, _| Some(Step::GpsFix(rng.percent(60)))),
    (1, |rng, _| Some(Step::Barometer(rng.percent(60)))),
    (4, |rng, setup| {
        setup
            .sensors
            .contains(&Sensor::Radar)
            .then(|| Step::Vehicle {
                range_m: rng.between(20..=140) as u16,
                closing_kmh: rng.between(5..=80) as u8,
                fast: rng.percent(30),
                side: rng.pick(&[Side::Behind, Side::Right, Side::Left]),
            })
    }),
    (2, |rng, _| {
        Some(Step::Battery {
            percent: rng.pick(&[100, 80, 40, 15, 8, 3]),
            charging: rng.percent(20),
        })
    }),
    (3, |_, _| Some(Step::PowerCut)),
    (2, |rng, _| {
        Some(Step::TornWrite {
            after: rng.below(12) as u8,
            partial: rng.below(600) as u16,
        })
    }),
    (2, |rng, setup| {
        (setup.card == Card::Ready).then(|| Step::CardRefusal {
            after: rng.below(12) as u8,
            partial: rng.below(600) as u16,
        })
    }),
];

fn ride(rng: &mut Rng) -> Step {
    Step::Ride {
        kmh: rng.between(3..=60) as u8,
        watts: rng.between(0..=600) as u16,
        bpm: rng.between(60..=190) as u8,
        rpm: rng.between(0..=120) as u8,
    }
}

fn step(rng: &mut Rng, setup: &Setup) -> Step {
    let total: u64 = WEIGHTS.iter().map(|(weight, _)| weight).sum();
    loop {
        let mut at = rng.below(total);
        let (_, draw) = WEIGHTS
            .iter()
            .find(|(weight, _)| {
                let found = at < *weight;
                at = at.saturating_sub(*weight);
                found
            })
            .expect("a row under the total");
        if let Some(step) = draw(rng, setup) {
            return step;
        }
    }
}

fn setup(rng: &mut Rng) -> Setup {
    let sensors = Sensor::ALL
        .into_iter()
        .filter(|_| rng.percent(55))
        .collect();
    let card = match rng.below(10) {
        0..=5 => Card::None,
        6..=8 => Card::Ready,
        _ => Card::Missing,
    };
    let center = rng.below(CENTERS.len() as u64) as u8;
    let course = match rng.below(10) {
        0..=3 => Course::Loop {
            center,
            length_m: rng.octaves(1_000..=40_000) as u32,
            climb_m: rng.below(300) as u16,
        },
        4..=6 => {
            let mut distance = 0;
            let knots = (0..rng.between(2..=6))
                .map(|_| {
                    distance += rng.between(500..=8_000) as u32;
                    (distance, rng.between(0..=3_000) as i16 - 50)
                })
                .collect();
            Course::Profile { center, knots }
        }
        _ => Course::Corpus {
            pick: rng.next_u64(),
        },
    };
    Setup {
        sensors,
        card,
        course,
    }
}

pub fn draw(seed: u64) -> Plan {
    let mut rng = Rng::new(seed);
    let setup = setup(&mut rng);
    let length_ms = rng.octaves(5 * 60_000..=90 * 60_000);
    let mut steps = Vec::new();
    if rng.percent(75) {
        steps.extend([ride(&mut rng), Step::Host(RideAction::Start)]);
    }
    let mut planned_ms = 0;
    while planned_ms < length_ms {
        let next = step(&mut rng, &setup);
        planned_ms += next.nominal_ms();
        steps.push(next);
    }
    Plan { seed, setup, steps }
}
