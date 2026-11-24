//! Today's workout: one codec file on the FILES volume at [`FILE`], put
//! there by a host (`cycling workout-load`) or the phone (`PUT
//! /api/workouts`). Each upload replaces the one before.
use super::codec::{self, ENCODED_MAX};
use super::model::Workout;
use device_api::bulk::{ReadWrite, SECTOR_SIZE};
use firmware_services::checksum::crc32;
use firmware_services::{files, volume};

pub const FILE: &str = "workouts/today.vw";

/// Why an upload was not stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    Decode(codec::DecodeError),
    /// The name is empty or only spaces.
    Unnamed,
    Load(LoadError),
}

/// Why today's workout could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// No volume table or no FILES volume on the medium.
    Volume(volume::Error),
    Files(files::Error),
    TooLarge,
    Decode(codec::DecodeError),
}

crate::kept::card_error!(LoadError);

impl LoadError {
    /// Short reason for a terminal reply.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Volume(_) => "no-volume",
            Self::Files(files::Error::Uninitialized) => "unformatted",
            Self::Files(_) => "files",
            Self::TooLarge => "too-large",
            Self::Decode(_) => "decode",
        }
    }
}

fn name() -> files::Name {
    files::Name::new(FILE).expect("a valid FILES name")
}

/// Today's workout with all its steps, `None` when none is stored.
pub fn read(media: &mut impl ReadWrite) -> Result<Option<Workout>, LoadError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
    let dir = files::mount(&mut store, &mut sector).map_err(LoadError::Files)?;
    let Some(entry) = dir
        .find(&mut store, &name(), &mut sector)
        .map_err(LoadError::Files)?
    else {
        return Ok(None);
    };
    let length = entry.length as usize;
    if length > ENCODED_MAX {
        return Err(LoadError::TooLarge);
    }
    let mut bytes = [0; ENCODED_MAX];
    let read = dir
        .read(&mut store, &entry, 0, &mut bytes[..length], &mut sector)
        .map_err(LoadError::Files)?;
    if read != length || crc32(&bytes[..length]) != entry.crc {
        return Err(LoadError::Files(files::Error::Mismatch));
    }
    codec::decode(&bytes[..length])
        .map(Some)
        .map_err(LoadError::Decode)
}

/// Store an uploaded codec file as today's workout. The bytes are decoded
/// and validated first, and only a whole valid file is written: the store
/// replaces a file in one commit, so a cut leaves the old one.
pub fn store(media: &mut impl ReadWrite, bytes: &[u8]) -> Result<Workout, StoreError> {
    let workout = codec::decode(bytes).map_err(StoreError::Decode)?;
    if workout.name.as_str().trim().is_empty() {
        return Err(StoreError::Unnamed);
    }
    let load = |error| StoreError::Load(LoadError::Files(error));
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector)
        .map_err(|error| StoreError::Load(LoadError::Volume(error)))?;
    files::write(&mut store, &name(), bytes, &mut sector).map_err(load)?;
    Ok(workout)
}

/// Delete today's workout: whether there was one.
pub fn clear(media: &mut impl ReadWrite) -> Result<bool, LoadError> {
    let mut sector = [0; SECTOR_SIZE];
    let mut store = volume::open(media, &volume::FILES, &mut sector).map_err(LoadError::Volume)?;
    match files::delete(&mut store, &name(), &mut sector) {
        Ok(()) => Ok(true),
        Err(files::Error::NotFound) => Ok(false),
        Err(error) => Err(LoadError::Files(error)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::workout::fixtures::{THRESHOLD, VO2_MAX};
    use device_api::bulk::{Error as MediaError, OwnedInfo};
    use std::collections::BTreeMap;

    /// A sparse owned region that starts blank.
    #[derive(Clone, Default)]
    pub(crate) struct Region(BTreeMap<u64, [u8; SECTOR_SIZE]>);

    impl ReadWrite for Region {
        fn entropy(&mut self) -> u64 {
            0
        }
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

    #[test]
    fn an_upload_replaces_the_last_and_clear_leaves_none() {
        let mut region = formatted();
        assert_eq!(read(&mut region), Ok(None));
        assert_eq!(
            store(&mut region, &codec::encode(&THRESHOLD)),
            Ok(THRESHOLD)
        );
        assert_eq!(read(&mut region), Ok(Some(THRESHOLD)));
        store(&mut region, &codec::encode(&VO2_MAX)).unwrap();
        assert_eq!(read(&mut region), Ok(Some(VO2_MAX)));
        assert_eq!(clear(&mut region), Ok(true));
        assert_eq!(read(&mut region), Ok(None));
        assert_eq!(clear(&mut region), Ok(false));
    }

    #[test]
    fn a_region_without_files_reads_as_an_error() {
        assert!(matches!(
            read(&mut Region::default()),
            Err(LoadError::Volume(_))
        ));
        let mut region = Region::default();
        let mut sector = [0; SECTOR_SIZE];
        volume::create(&mut region, &mut sector).unwrap();
        assert_eq!(
            read(&mut region),
            Err(LoadError::Files(files::Error::Uninitialized))
        );
    }

    #[test]
    fn a_damaged_file_left_by_a_host_reads_as_an_error() {
        let mut region = formatted();
        let mut damaged = codec::encode(&VO2_MAX);
        damaged[5] ^= 1;
        put(&mut region, FILE, &damaged);
        assert!(matches!(read(&mut region), Err(LoadError::Decode(_))));
    }

    #[test]
    fn store_keeps_whole_valid_workouts_and_refuses_the_rest_untouched() {
        let mut region = formatted();
        let good = codec::encode(&THRESHOLD);
        store(&mut region, &good).unwrap();
        let mut cut = good.to_vec();
        cut.pop();
        let mut flipped = good.to_vec();
        flipped[8] ^= 4;
        for bad in [&cut[..], &flipped[..], &[][..], b"VW"] {
            assert!(matches!(
                store(&mut region, bad),
                Err(StoreError::Decode(_))
            ));
        }
        assert_eq!(read(&mut region), Ok(Some(THRESHOLD)));
    }

    /// What the phone page's JavaScript and `cycling workout-load` both make
    /// of the JSON in `workouts/load.py`'s docstring, with an open step, a
    /// ramp, distances and a repeat group: the device reads it as written.
    #[test]
    fn a_host_encoded_workout_is_stored_and_read_back() {
        const HEX: &str = "565702000f4f5645522022554e4445525322203f0801000058020000013200410000000000010078000000015c00600000000005554e44455201003c000000022c0140010000000002005a00000001280032000000000001020000000006640014003c000000010200000000055a006400000000000301d0070000030100000000000000030196000000043c00460000000000010203386ddb3c";
        let bytes: std::vec::Vec<u8> = (0..HEX.len() / 2)
            .map(|i| u8::from_str_radix(&HEX[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let mut region = formatted();
        store(&mut region, &bytes).unwrap();
        let workout = read(&mut region).unwrap().unwrap();
        assert_eq!(workout.name.as_str(), "OVER \"UNDERS\" ?");
        assert!(workout.has_untimed_steps());
        assert_eq!(workout.steps().len(), 8);
        assert_eq!(&codec::encode(&workout)[..], &bytes[..]);
    }
}
