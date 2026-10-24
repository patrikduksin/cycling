//! The workouts a rider can pick: built-ins that ship in flash and always
//! work, then workouts a host uploaded to the FILES volume under
//! [`FOLDER`].
//!
//! `Library` keeps only a small index of the uploads (file, name, outline);
//! a ride loads its workout's steps from FILES when it starts (`load`).
use super::codec::{self, ENCODED_MAX};
use super::model::{Block, Intensity::*, Length, Name, Step, Target, Test, Workout};
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::crc32;
use firmware_services::{files, volume};

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
    "ENDURANCE Z2 60",
    &[
        Step::new(Warmup, min(10), ftp(50, 60)),
        Step::new(Work, min(60), ftp(65, 75)).note("CONVERSATIONAL"),
        Step::new(Cooldown, min(5), ftp(40, 55)),
    ],
    &[],
    None,
);

/// Easy legs: spin loosely under the aerobic floor.
pub const RECOVERY: Workout = Workout::new(
    "RECOVERY SPIN 30",
    &[
        Step::new(Warmup, min(5), ftp(40, 50)),
        Step::new(Work, min(30), ftp(45, 55)).note("LIGHT, 90+ RPM"),
        Step::new(Cooldown, min(5), ftp(40, 50)),
    ],
    &[],
    None,
);

pub const ENDURANCE_LONG: Workout = Workout::new(
    "ENDURANCE Z2 90",
    &[
        Step::new(Warmup, min(10), ftp(50, 60)),
        Step::new(Work, min(90), ftp(65, 75)).note("EAT AND DRINK"),
        Step::new(Cooldown, min(5), ftp(40, 55)),
    ],
    &[],
    None,
);

pub const TEMPO: Workout = Workout::new(
    "TEMPO 2X20",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, min(20), ftp(80, 88)).note("SMOOTH, 85-90 RPM"),
        Step::new(Recovery, min(5), ftp(45, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 2)],
    None,
);

pub const SWEET_SPOT_SHORT: Workout = Workout::new(
    "SWEET SPOT 3X12",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, min(12), ftp(88, 94)).note("STEADY, SEATED"),
        Step::new(Recovery, min(4), ftp(45, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 3)],
    None,
);

/// Two sets of five: two minutes just under threshold, two just over.
pub const OVER_UNDERS: Workout = Workout::new(
    "OVER UNDER 2X20",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, min(2), ftp(93, 97)).note("UNDER, SETTLE IN"),
        Step::new(Work, min(2), ftp(103, 107)).note("OVER, HOLD ON"),
        Step::new(Recovery, min(5), ftp(45, 55)),
        Step::new(Work, min(2), ftp(93, 97)).note("UNDER, SETTLE IN"),
        Step::new(Work, min(2), ftp(103, 107)).note("OVER, HOLD ON"),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 5), block(4, 2, 5)],
    None,
);

pub const VO2_LONG: Workout = Workout::new(
    "VO2 MAX 6X4",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, min(4), ftp(105, 115)).note("STRONG, EVEN PACE"),
        Step::new(Recovery, min(4), ftp(40, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 6)],
    None,
);

/// Three sets of ten: thirty seconds hard, thirty easy, five minutes between.
pub const VO2_THIRTY: Workout = Workout::new(
    "VO2 MAX 30-30S",
    &[
        Step::new(Warmup, min(12), ftp(50, 70)),
        Step::new(Work, secs(30), ftp(120, 130)).note("HARD, 100+ RPM"),
        Step::new(Recovery, secs(30), ftp(40, 50)),
        Step::new(Recovery, min(5), ftp(45, 55)),
        Step::new(Work, secs(30), ftp(120, 130)).note("HARD, 100+ RPM"),
        Step::new(Recovery, secs(30), ftp(40, 50)),
        Step::new(Recovery, min(5), ftp(45, 55)),
        Step::new(Work, secs(30), ftp(120, 130)).note("HARD, 100+ RPM"),
        Step::new(Recovery, secs(30), ftp(40, 50)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 10), block(4, 2, 10), block(7, 2, 10)],
    None,
);

pub const ANAEROBIC: Workout = Workout::new(
    "ANAEROBIC 6X1",
    &[
        Step::new(Warmup, min(15), ftp(50, 70)),
        Step::new(Work, min(1), ftp(150, 170)).note("ALL IN"),
        Step::new(Recovery, min(4), ftp(40, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 6)],
    None,
);

pub const SPRINTS: Workout = Workout::new(
    "SPRINTS 10X10S",
    &[
        Step::new(Warmup, min(15), ftp(50, 70)),
        Step::new(Work, secs(10), ftp(200, 250)).note("MAX, FROM A ROLL"),
        Step::new(Recovery, secs(120), ftp(40, 55)),
        Step::new(Cooldown, min(10), ftp(40, 55)),
    ],
    &[block(1, 2, 10)],
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

/// Built-in workouts in picker order: by rising intensity, then the tests.
pub const BUILT_IN: [&Workout; 16] = [
    &RECOVERY,
    &ENDURANCE,
    &ENDURANCE_LONG,
    &TEMPO,
    &SWEET_SPOT_SHORT,
    &SWEET_SPOT,
    &THRESHOLD,
    &OVER_UNDERS,
    &VO2_MAX,
    &VO2_LONG,
    &VO2_THIRTY,
    &ANAEROBIC,
    &SPRINTS,
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

/// The folder of uploaded workouts on the FILES volume. A host puts each
/// as one codec file (`mise run workout-load`); the library lists them.
pub const FOLDER: &str = "workouts/";
/// Uploads the picker lists, in file-name order; later ones are not shown.
pub const UPLOADS: usize = 8;

/// What the picker shows of a workout without decoding it again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outline {
    pub planned_secs: u32,
    pub played: u8,
    pub test: Option<Test>,
    pub untimed: bool,
}

impl Outline {
    pub fn of(workout: &Workout) -> Self {
        Self {
            planned_secs: workout.planned_secs(),
            played: workout.played_len().min(usize::from(u8::MAX)) as u8,
            test: workout.test,
            untimed: workout.has_untimed_steps(),
        }
    }
}

/// An uploaded workout the library found: its file, and what it shows. The
/// steps stay on FILES until a ride plays it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upload {
    pub file: files::Name,
    pub name: Name,
    pub outline: Outline,
}

/// Why uploads could not be listed or a workout loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// No volume table or no FILES volume on the medium.
    Volume(volume::Error),
    Files(files::Error),
    /// The upload is gone since the library listed it.
    Missing,
    TooLarge,
    Decode(codec::DecodeError),
}

impl LoadError {
    /// Short reason for a terminal reply.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Volume(_) => "no-volume",
            Self::Files(files::Error::Uninitialized) => "unformatted",
            Self::Files(_) => "files",
            Self::Missing => "missing",
            Self::TooLarge => "too-large",
            Self::Decode(_) => "decode",
        }
    }
}

/// Built-ins, and the uploads FILES held when last listed.
#[derive(Default)]
pub struct Library {
    uploads: heapless::Vec<Upload, UPLOADS>,
    /// Upload files that did not decode, or did not fit, at the last listing.
    pub skipped: u8,
}

impl Library {
    /// Every workout in picker order: built-ins, then uploads by file name.
    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        (0..BUILT_IN.len() as u8)
            .map(Id::BuiltIn)
            .chain((0..self.uploads.len() as u8).map(Id::Uploaded))
    }

    pub fn uploads(&self) -> &[Upload] {
        &self.uploads
    }

    /// A workout's name and outline.
    pub fn outline(&self, id: Id) -> Option<(Name, Outline)> {
        match id {
            Id::BuiltIn(index) => BUILT_IN
                .get(usize::from(index))
                .map(|workout| (workout.name, Outline::of(workout))),
            Id::Uploaded(index) => self
                .uploads
                .get(usize::from(index))
                .map(|upload| (upload.name, upload.outline)),
        }
    }

    /// The upload stored as `file`, if listed.
    pub fn find(&self, file: &files::Name) -> Option<Id> {
        let index = self.uploads.iter().position(|u| u.file == *file)?;
        Some(Id::Uploaded(index as u8))
    }

    /// List the uploads on `media`'s FILES volume again, decoding each once
    /// for its name and outline. On failure the library holds no uploads.
    pub fn refresh(&mut self, media: &mut impl ReadWrite) -> Result<(), LoadError> {
        self.uploads.clear();
        self.skipped = 0;
        let mut sector = [0; SECTOR_SIZE];
        let mut store =
            volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
        let dir = files::mount(&mut store, &mut sector).map_err(LoadError::Files)?;
        let mut found: heapless::Vec<files::Entry, UPLOADS> = heapless::Vec::new();
        let mut skipped = 0u8;
        dir.list(&mut store, &mut sector, |entry| {
            if !entry.name.as_str().starts_with(FOLDER) {
                return;
            }
            // Keep the first names in order, whatever the slot order.
            let at = found.partition_point(|e| e.name.as_str() < entry.name.as_str());
            if found.is_full() {
                skipped = skipped.saturating_add(1);
                if at == found.len() {
                    return;
                }
                found.pop();
            }
            let _ = found.insert(at, *entry);
        })
        .map_err(LoadError::Files)?;
        self.skipped = skipped;
        for entry in &found {
            match read(&dir, &mut store, entry, &mut sector) {
                Ok(workout) => {
                    let _ = self.uploads.push(Upload {
                        file: entry.name,
                        name: workout.name,
                        outline: Outline::of(&workout),
                    });
                }
                Err(_) => self.skipped = self.skipped.saturating_add(1),
            }
        }
        Ok(())
    }

    /// The workout `id` with all its steps: a built-in, or an upload read
    /// from `media` now.
    pub fn load(&self, id: Id, media: &mut impl ReadWrite) -> Result<Workout, LoadError> {
        let index = match id {
            Id::BuiltIn(index) => {
                return BUILT_IN
                    .get(usize::from(index))
                    .map(|workout| **workout)
                    .ok_or(LoadError::Missing);
            }
            Id::Uploaded(index) => usize::from(index),
        };
        let upload = self.uploads.get(index).ok_or(LoadError::Missing)?;
        let mut sector = [0; SECTOR_SIZE];
        let mut store =
            volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
        let dir = files::mount(&mut store, &mut sector).map_err(LoadError::Files)?;
        let entry = dir
            .find(&mut store, &upload.file, &mut sector)
            .map_err(LoadError::Files)?
            .ok_or(LoadError::Missing)?;
        read(&dir, &mut store, &entry, &mut sector)
    }

    /// Delete upload `id`'s file and drop it from the list.
    pub fn delete(&mut self, id: Id, media: &mut impl ReadWrite) -> Result<(), LoadError> {
        let Id::Uploaded(index) = id else {
            return Err(LoadError::Missing);
        };
        let upload = self
            .uploads
            .get(usize::from(index))
            .ok_or(LoadError::Missing)?;
        let mut sector = [0; SECTOR_SIZE];
        let mut store =
            volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
        match files::delete(&mut store, &upload.file, &mut sector) {
            Ok(()) | Err(files::Error::NotFound) => {}
            Err(error) => return Err(LoadError::Files(error)),
        }
        self.uploads.remove(usize::from(index));
        Ok(())
    }
}

/// Read and decode one workout file, checking its CRC on the way.
fn read(
    dir: &files::Dir,
    media: &mut impl ReadWrite,
    entry: &files::Entry,
    sector: &mut files::Sector,
) -> Result<Workout, LoadError> {
    let length = entry.length as usize;
    if length > ENCODED_MAX {
        return Err(LoadError::TooLarge);
    }
    let mut bytes = [0; ENCODED_MAX];
    let read = dir
        .read(media, entry, 0, &mut bytes[..length], sector)
        .map_err(LoadError::Files)?;
    if read != length || crc32(&bytes[..length]) != entry.crc {
        return Err(LoadError::Files(files::Error::Mismatch));
    }
    codec::decode(&bytes[..length]).map_err(LoadError::Decode)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::model::{MAX_PLAYED, NAME_BYTES};
    use super::*;
    use device_api::bulk::{Error as MediaError, OwnedInfo};
    use std::collections::BTreeMap;

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
        assert_eq!(VO2_THIRTY.played_len(), 64);
        assert_eq!(OPENERS.played_len(), 14);
        assert_eq!(
            FTP_TWENTY.planned_secs(),
            (10 + 6 + 3 + 5 + 5 + 20 + 10) * 60
        );
    }

    /// Each built-in's name token, and the seconds its name promises at the
    /// hard part: work steps, or the measured step of a test. A ramp has no
    /// fixed length.
    const PROMISED: [(&Workout, &str, u32); 16] = [
        (&RECOVERY, "SPIN 30", 30 * 60),
        (&ENDURANCE, "Z2 60", 60 * 60),
        (&ENDURANCE_LONG, "Z2 90", 90 * 60),
        (&TEMPO, "2X20", 2 * 20 * 60),
        (&SWEET_SPOT_SHORT, "3X12", 3 * 12 * 60),
        (&SWEET_SPOT, "2X20", 2 * 20 * 60),
        (&THRESHOLD, "4X8", 4 * 8 * 60),
        (&OVER_UNDERS, "2X20", 2 * 20 * 60),
        (&VO2_MAX, "5X3", 5 * 3 * 60),
        (&VO2_LONG, "6X4", 6 * 4 * 60),
        (&VO2_THIRTY, "30-30S", 3 * 10 * 30),
        (&ANAEROBIC, "6X1", 6 * 60),
        (&SPRINTS, "10X10S", 10 * 10),
        (&OPENERS, "OPENERS", 3 * 75),
        (&FTP_TWENTY, "20 MIN", 20 * 60),
        (&FTP_RAMP, "RAMP", 0),
    ];

    #[test]
    fn every_built_in_is_listed_once_and_promises_what_it_rides() {
        assert_eq!(PROMISED.len(), BUILT_IN.len());
        for (index, (workout, token, promised)) in PROMISED.iter().enumerate() {
            let name = workout.name.as_str();
            assert!(core::ptr::eq(*workout, BUILT_IN[index]), "{name}: order");
            assert!(name.contains(token), "{name} lacks {token}");
            assert!(name.len() <= 16, "{name} is too long for a card");
            for other in &BUILT_IN[..index] {
                assert_ne!(other.name, workout.name, "{name} twice");
            }
            let steps = workout.steps();
            assert_eq!(steps[0].intensity, Warmup, "{name}: starts warm");
            assert_eq!(
                steps[steps.len() - 1].intensity,
                Cooldown,
                "{name}: ends cool"
            );
            let measured = if workout.test.is_some() { Test } else { Work };
            let mut hard = 0;
            let mut planned = 0;
            for played in workout.played() {
                let step = played.step;
                assert!(step.note.as_bytes().len() < NAME_BYTES, "{name}: note cut");
                match (step.length, step.target) {
                    (Length::Time(secs), _) => {
                        assert!(secs > 0, "{name}: empty step");
                        planned += u32::from(secs);
                        if step.intensity == measured {
                            hard += u32::from(secs);
                        }
                    }
                    (Length::Open, Target::Ramp { .. }) => {}
                    other => panic!("{name}: {other:?}"),
                }
                if let Target::Ftp { low, high } = step.target {
                    assert!(
                        (35..=250).contains(&low) && low <= high && high <= 250,
                        "{name}"
                    );
                    match step.intensity {
                        Work => assert!(low >= 45, "{name}: work too easy"),
                        Recovery | Warmup | Cooldown => {
                            assert!(high <= 70, "{name}: {:?} too hard", step.intensity)
                        }
                        Test => assert!(low >= 100, "{name}: test too easy"),
                    }
                } else {
                    assert!(
                        matches!(step.target, Target::Free | Target::Ramp { .. })
                            && step.intensity == Test,
                        "{name}: only tests ride free or ramp"
                    );
                }
            }
            assert_eq!(hard, *promised, "{name}: hard seconds");
            assert_eq!(planned, workout.planned_secs(), "{name}: planned");
            assert!(
                planned <= 3 * 3600 && workout.played_len() <= MAX_PLAYED,
                "{name}"
            );
            assert!(
                planned >= 20 * 60 || workout.has_untimed_steps(),
                "{name}: too short to be a session"
            );
        }
    }

    /// A sparse owned region that starts blank.
    #[derive(Default)]
    pub(crate) struct Region(BTreeMap<u64, [u8; SECTOR_SIZE]>);

    impl ReadWrite for Region {
        fn owned_info(&mut self) -> Result<OwnedInfo, MediaError> {
            Ok(OwnedInfo {
                start_sector: 4096,
                sectors: 8_000_000,
                sector_size: SECTOR_SIZE as u16,
            })
        }
        fn owned_read(
            &mut self,
            index: u64,
            out: &mut [u8; SECTOR_SIZE],
        ) -> Result<(), MediaError> {
            *out = self.0.get(&index).copied().unwrap_or([0; SECTOR_SIZE]);
            Ok(())
        }
        fn owned_write(&mut self, index: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), MediaError> {
            self.0.insert(index, *input);
            Ok(())
        }
    }

    /// A region with a volume table and a formatted FILES volume.
    pub(crate) fn formatted() -> Region {
        let mut region = Region::default();
        let mut sector = [0; SECTOR_SIZE];
        volume::create(&mut region, &mut sector).unwrap();
        let mut store = volume::open(&mut region, &volume::FILES, &mut sector).unwrap();
        files::format(&mut store, 7, &mut sector).unwrap();
        region
    }

    pub(crate) fn put(region: &mut Region, name: &str, bytes: &[u8]) {
        let mut sector = [0; SECTOR_SIZE];
        let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
        let name = files::Name::new(name).unwrap();
        files::write(&mut store, &name, bytes, &mut sector).unwrap();
    }

    fn remove(region: &mut Region, name: &str) {
        let mut sector = [0; SECTOR_SIZE];
        let mut store = volume::open(region, &volume::FILES, &mut sector).unwrap();
        let name = files::Name::new(name).unwrap();
        files::delete(&mut store, &name, &mut sector).unwrap();
    }

    #[test]
    fn uploads_list_by_name_load_on_demand_and_delete() {
        let mut region = formatted();
        let mut library = Library::default();
        library.refresh(&mut region).unwrap();
        assert_eq!(library.ids().count(), BUILT_IN.len());
        put(&mut region, "workouts/b.vw", &codec::encode(&VO2_MAX));
        put(&mut region, "workouts/a.vw", &codec::encode(&THRESHOLD));
        put(&mut region, "other/c.vw", &codec::encode(&SWEET_SPOT));
        let mut damaged = codec::encode(&OPENERS);
        damaged[5] ^= 1;
        put(&mut region, "workouts/d.vw", &damaged);
        library.refresh(&mut region).unwrap();
        assert_eq!(library.skipped, 1);
        let first = Id::Uploaded(0);
        assert_eq!(library.ids().count(), BUILT_IN.len() + 2);
        assert_eq!(
            library.outline(first),
            Some((THRESHOLD.name, Outline::of(&THRESHOLD)))
        );
        assert_eq!(library.load(Id::Uploaded(1), &mut region), Ok(VO2_MAX));
        assert_eq!(library.load(Id::BuiltIn(5), &mut region), Ok(SWEET_SPOT));
        assert_eq!(
            library.find(&files::Name::new("workouts/b.vw").unwrap()),
            Some(Id::Uploaded(1))
        );
        library.delete(first, &mut region).unwrap();
        assert_eq!(
            library.outline(first).map(|(name, _)| name),
            Some(VO2_MAX.name)
        );
        library.refresh(&mut region).unwrap();
        assert_eq!(library.uploads().len(), 1);
        // A file removed behind the library's back loads as missing.
        remove(&mut region, "workouts/b.vw");
        assert_eq!(library.load(first, &mut region), Err(LoadError::Missing));
    }

    #[test]
    fn a_region_without_files_lists_built_ins_only() {
        let mut library = Library::default();
        let mut blank = Region::default();
        assert!(matches!(
            library.refresh(&mut blank),
            Err(LoadError::Volume(_))
        ));
        let mut region = Region::default();
        let mut sector = [0; SECTOR_SIZE];
        volume::create(&mut region, &mut sector).unwrap();
        assert_eq!(
            library.refresh(&mut region),
            Err(LoadError::Files(files::Error::Uninitialized))
        );
        assert_eq!(library.ids().count(), BUILT_IN.len());
    }

    #[test]
    fn only_the_first_uploads_by_name_are_listed() {
        let mut region = formatted();
        for index in (0..UPLOADS + 2).rev() {
            let name = std::format!("workouts/{index:02}");
            put(&mut region, &name, &codec::encode(&VO2_MAX));
        }
        let mut library = Library::default();
        library.refresh(&mut region).unwrap();
        assert_eq!(library.uploads().len(), UPLOADS);
        assert_eq!(library.skipped, 2);
        assert_eq!(library.uploads()[0].file.as_str(), "workouts/00");
        assert_eq!(library.uploads()[UPLOADS - 1].file.as_str(), "workouts/07");
    }
}
