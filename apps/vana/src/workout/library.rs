//! The workouts a rider can pick: built-ins that ship in flash and always
//! work, then workouts loaded from a host into a small store.
//!
//! `Library` hides where uploads live behind [`WorkoutStore`], a slot store
//! of encoded workouts (see `codec`). This build has no store
//! ([`Volatile`]): uploads last until power-off; a persistent volume
//! implements the same trait. Uploads go in through `install`, from the terminal
//! (`WORKOUT BEGIN/DATA/COMMIT`) or later the phone companion.
use super::codec::{self, ENCODED_MAX};
use super::model::{Block, Intensity::*, Length, Step, Target, Test, Workout};

/// Workouts a store keeps besides the built-ins.
pub const UPLOAD_SLOTS: usize = 4;

const fn ftp(low: u8, high: u8) -> Target {
    Target::Ftp { low, high }
}
const fn min(minutes: u16) -> Length {
    Length::Time(minutes * 60)
}
const fn block(first: u8, len: u8, count: u8) -> Block {
    Block { first, len, count }
}

pub const SWEET_SPOT: Workout = Workout::new(
    "SWEET SPOT 2X20",
    &[
        Step::new(Warmup, min(10), ftp(50, 65)),
        Step::new(Work, min(20), ftp(88, 94)).note("STEADY, SEATED"),
        Step::new(Recovery, min(5), ftp(45, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 2)],
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

pub const ENDURANCE: Workout = Workout::new(
    "ENDURANCE Z2",
    &[
        Step::new(Warmup, min(10), ftp(50, 60)),
        Step::new(Work, min(60), ftp(65, 75)).note("CONVERSATIONAL"),
        Step::new(Cooldown, min(5), ftp(40, 55)),
    ],
    &[],
    None,
);

pub const OPENERS: Workout = Workout::new(
    "OPENERS",
    &[
        Step::new(Warmup, min(15), ftp(50, 65)),
        Step::new(Work, min(1), ftp(110, 120)),
        Step::new(Recovery, min(2), ftp(40, 55)),
        Step::new(Work, Length::Time(15), ftp(150, 200)).note("SPRINT"),
        Step::new(Recovery, Length::Time(105), ftp(40, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 3), block(3, 2, 3)],
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

/// One-minute steps 20 W apart from 100 W until the rider cannot hold one.
/// FTP is 75 % of the best minute.
pub const FTP_RAMP: Workout = Workout::new(
    "FTP RAMP TEST",
    &[
        Step::new(Warmup, min(5), ftp(45, 55)),
        Step::new(
            Test,
            Length::Open,
            Target::Ramp {
                start: 100,
                step: 20,
                every: 60,
            },
        )
        .note("HOLD EACH STEP"),
        Step::new(Cooldown, min(5), ftp(40, 50)),
    ],
    &[],
    Some(Test::Ramp),
);

/// Built-in workouts in picker order: training first, then the tests.
pub const BUILT_IN: [&Workout; 7] = [
    &SWEET_SPOT,
    &VO2_MAX,
    &THRESHOLD,
    &ENDURANCE,
    &OPENERS,
    &FTP_TWENTY,
    &FTP_RAMP,
];

/// A workout by where it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    BuiltIn(u8),
    Uploaded(u8),
}

/// The store failed, or a slot holds bytes that do not decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    Bounds,
    Failed,
}

/// Slots of encoded workouts. A read reports the bytes in the slot, zero for
/// an empty one; a write replaces the slot. Completion means the bytes are
/// stored.
pub trait WorkoutStore {
    fn read(&mut self, slot: usize, output: &mut [u8; ENCODED_MAX]) -> Result<usize, StoreError>;
    fn write(&mut self, slot: usize, bytes: &[u8]) -> Result<(), StoreError>;
    fn erase(&mut self, slot: usize) -> Result<(), StoreError>;
}

/// No store: uploads live only in the library's decoded copy, lost at
/// power-off. A persistent volume implements the trait instead.
#[derive(Default)]
pub struct Volatile;

impl WorkoutStore for Volatile {
    fn read(&mut self, slot: usize, _: &mut [u8; ENCODED_MAX]) -> Result<usize, StoreError> {
        in_bounds(slot).map(|()| 0)
    }

    fn write(&mut self, slot: usize, _: &[u8]) -> Result<(), StoreError> {
        in_bounds(slot)
    }

    fn erase(&mut self, slot: usize) -> Result<(), StoreError> {
        in_bounds(slot)
    }
}

fn in_bounds(slot: usize) -> Result<(), StoreError> {
    if slot < UPLOAD_SLOTS {
        Ok(())
    } else {
        Err(StoreError::Bounds)
    }
}

/// Why an upload was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallError {
    Bounds,
    Decode(codec::DecodeError),
    Store(StoreError),
}

/// Built-ins and the decoded uploads of a store.
pub struct Library<S = Volatile> {
    store: S,
    uploaded: [Option<Workout>; UPLOAD_SLOTS],
}

impl Default for Library<Volatile> {
    fn default() -> Self {
        Self::new(Volatile)
    }
}

impl<S: WorkoutStore> Library<S> {
    /// A library over `store`, decoding what it already holds; slots that
    /// fail to read or decode show as empty and are left untouched.
    pub fn new(mut store: S) -> Self {
        let mut uploaded = [None; UPLOAD_SLOTS];
        let mut bytes = [0; ENCODED_MAX];
        for (slot, workout) in uploaded.iter_mut().enumerate() {
            if let Ok(len) = store.read(slot, &mut bytes)
                && len > 0
            {
                *workout = codec::decode(&bytes[..len]).ok();
            }
        }
        Self { store, uploaded }
    }

    /// Every workout in picker order: built-ins, then uploads by slot.
    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        (0..BUILT_IN.len() as u8).map(Id::BuiltIn).chain(
            self.uploaded
                .iter()
                .enumerate()
                .filter(|(_, w)| w.is_some())
                .map(|(slot, _)| Id::Uploaded(slot as u8)),
        )
    }

    pub fn get(&self, id: Id) -> Option<&Workout> {
        match id {
            Id::BuiltIn(index) => BUILT_IN.get(usize::from(index)).copied(),
            Id::Uploaded(slot) => self.uploaded.get(usize::from(slot))?.as_ref(),
        }
    }

    /// Decode, check and store `bytes` in `slot`, replacing what it held.
    pub fn install(&mut self, slot: usize, bytes: &[u8]) -> Result<&Workout, InstallError> {
        if slot >= UPLOAD_SLOTS {
            return Err(InstallError::Bounds);
        }
        let workout = codec::decode(bytes).map_err(InstallError::Decode)?;
        self.store.write(slot, bytes).map_err(InstallError::Store)?;
        Ok(self.uploaded[slot].insert(workout))
    }

    pub fn remove(&mut self, slot: usize) -> Result<(), InstallError> {
        if slot >= UPLOAD_SLOTS {
            return Err(InstallError::Bounds);
        }
        self.store.erase(slot).map_err(InstallError::Store)?;
        self.uploaded[slot] = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_are_valid_and_tests_measure_one_step() {
        for workout in BUILT_IN {
            assert_eq!(workout.validate(), Ok(()), "{:?}", workout.name);
            let measured = workout
                .steps()
                .iter()
                .filter(|step| step.intensity == super::super::model::Intensity::Test)
                .count();
            assert_eq!(
                measured,
                usize::from(workout.test.is_some()),
                "{:?}",
                workout.name
            );
        }
        assert_eq!(SWEET_SPOT.planned_secs(), 70 * 60);
        assert_eq!(VO2_MAX.played_len(), 12);
        assert_eq!(OPENERS.played_len(), 14);
        assert_eq!(
            FTP_TWENTY.planned_secs(),
            (10 + 6 + 3 + 5 + 5 + 20 + 10) * 60
        );
    }

    /// A store that keeps its bytes, as a persistent volume does.
    #[derive(Default)]
    struct Kept([std::vec::Vec<u8>; UPLOAD_SLOTS]);

    impl WorkoutStore for Kept {
        fn read(&mut self, slot: usize, out: &mut [u8; ENCODED_MAX]) -> Result<usize, StoreError> {
            let bytes = self.0.get(slot).ok_or(StoreError::Bounds)?;
            out[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        }
        fn write(&mut self, slot: usize, bytes: &[u8]) -> Result<(), StoreError> {
            *self.0.get_mut(slot).ok_or(StoreError::Bounds)? = bytes.to_vec();
            Ok(())
        }
        fn erase(&mut self, slot: usize) -> Result<(), StoreError> {
            self.write(slot, &[])
        }
    }

    #[test]
    fn uploads_install_list_and_remove() {
        let mut library = Library::new(Kept::default());
        assert_eq!(library.ids().count(), BUILT_IN.len());
        let encoded = codec::encode(&VO2_MAX);
        let installed = library.install(2, &encoded).unwrap();
        assert_eq!(installed, &VO2_MAX);
        assert_eq!(library.ids().last(), Some(Id::Uploaded(2)));
        assert_eq!(library.get(Id::Uploaded(2)), Some(&VO2_MAX));
        assert_eq!(
            library.install(UPLOAD_SLOTS, &encoded),
            Err(InstallError::Bounds)
        );
        let mut damaged = encoded.clone();
        damaged[5] ^= 1;
        assert!(matches!(
            library.install(1, &damaged),
            Err(InstallError::Decode(_))
        ));
        assert_eq!(library.get(Id::Uploaded(1)), None);
        // A library over the same store decodes what it holds.
        let reopened = Library::new(library.store);
        assert_eq!(reopened.get(Id::Uploaded(2)), Some(&VO2_MAX));
        let mut library = reopened;
        library.remove(2).unwrap();
        assert_eq!(library.ids().count(), BUILT_IN.len());
    }
}
