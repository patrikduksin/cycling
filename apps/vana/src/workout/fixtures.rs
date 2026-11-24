//! Workouts the tests ride. None ships on the device: a rider's workout
//! comes from a host or the phone (`workout::today`).
use super::model::{Block, Intensity::*, Length, Step, Target, Test, Workout};

const fn ftp(low: u8, high: u8) -> Target {
    Target::Ftp { low, high }
}
const fn min(minutes: u16) -> Length {
    Length::Time(minutes * 60)
}
const fn secs(seconds: u16) -> Length {
    Length::Time(seconds)
}
const fn block(first: u8, len: u8, count: u8) -> Block {
    Block { first, len, count }
}

pub const ENDURANCE: Workout = Workout::new(
    "ENDURANCE Z2 60",
    &[
        Step::new(Warmup, min(10), ftp(50, 60)),
        Step::new(Work, min(60), ftp(65, 75)).note("CONVERSATIONAL"),
        Step::new(Cooldown, min(5), ftp(40, 55)),
    ],
    &[],
    None,
);

pub const THRESHOLD: Workout = Workout::new(
    "THRESHOLD 4X8",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, min(8), ftp(95, 105)).note("AT THRESHOLD"),
        Step::new(Recovery, min(4), ftp(45, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 4)],
    None,
);

pub const VO2_MAX: Workout = Workout::new(
    "VO2 MAX 5X3",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, min(3), ftp(106, 120)).note("HARD, HIGH CADENCE"),
        Step::new(Recovery, min(3), ftp(40, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 5)],
    None,
);

/// Two sets of ten: thirty seconds hard, thirty easy, the last easy half
/// minute of a set joined to the five-minute break. Three sets would need
/// more steps than a workout holds.
pub const VO2_THIRTY: Workout = Workout::new(
    "VO2 MAX 30-30S",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, secs(30), ftp(120, 130)).note("HARD, 100+ RPM"),
        Step::new(Recovery, secs(30), ftp(40, 50)),
        Step::new(Work, secs(30), ftp(120, 130)).note("HARD, 100+ RPM"),
        Step::new(Recovery, min(5), ftp(45, 55)),
        Step::new(Work, secs(30), ftp(120, 130)).note("HARD, 100+ RPM"),
        Step::new(Recovery, secs(30), ftp(40, 50)),
        Step::new(Work, secs(30), ftp(120, 130)).note("HARD, 100+ RPM"),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 9), block(5, 2, 9)],
    None,
);

pub const SPRINTS: Workout = Workout::new(
    "SPRINTS 10X10S",
    &[
        Step::new(Warmup, min(15), ftp(50, 70)),
        Step::new(Work, secs(10), Target::Free).note("MAX, FROM A ROLL"),
        Step::new(Recovery, secs(120), ftp(40, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 10)],
    None,
);

/// Warm up with spin-ups and a five-minute blow-out, recover, ride twenty
/// minutes as hard as can be held, cool down. FTP is 95 % of the twenty.
pub const FTP_TWENTY: Workout = Workout::new(
    "FTP TEST 20 MIN",
    &[
        Step::new(Warmup, min(10), ftp(50, 65)),
        Step::new(Work, min(1), ftp(100, 110)).note("FAST SPIN"),
        Step::new(Recovery, min(1), ftp(45, 55)),
        Step::new(Warmup, min(3), ftp(50, 60)),
        Step::new(Work, min(5), ftp(100, 110)).note("BLOW OUT"),
        Step::new(Recovery, min(5), ftp(45, 55)),
        Step::new(Test, min(20), Target::Free).note("ALL OUT, EVEN PACE"),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 3)],
    Some(Test::Twenty),
);

/// An open warm-up, then one-minute steps from 50 % FTP, 6 % more each, from
/// the lap press until the rider cannot hold one. FTP is 75 % of the best
/// minute.
pub const FTP_RAMP: Workout = Workout::new(
    "FTP RAMP TEST",
    &[
        Step::new(Warmup, Length::Open, ftp(45, 55)),
        Step::new(
            Test,
            Length::Open,
            Target::FtpRamp {
                start: 50,
                step: 6,
                every: 60,
            },
        )
        .note("HOLD EACH STEP"),
    ],
    &[],
    Some(Test::Ramp),
);

/// Every fixture, for tests that try each.
pub const ALL: [&Workout; 7] = [
    &ENDURANCE,
    &THRESHOLD,
    &VO2_MAX,
    &VO2_THIRTY,
    &SPRINTS,
    &FTP_TWENTY,
    &FTP_RAMP,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_are_valid_and_tests_measure_one_step() {
        for workout in ALL {
            assert_eq!(workout.validate(), Ok(()), "{:?}", workout.name);
            let measured = workout
                .steps()
                .iter()
                .filter(|step| step.intensity == Test)
                .count();
            assert_eq!(
                measured,
                usize::from(workout.test.is_some()),
                "{:?}",
                workout.name
            );
        }
    }
}
