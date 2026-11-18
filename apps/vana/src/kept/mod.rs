//! What VANA keeps on the FILES volume beside the ride journals: the rider's
//! bests, each saved ride's facts, the rides the rider hid and the course.
//!
//! Each has its record, its codec and the state machine that reads it into
//! memory and writes it back. `StoreError` is the typed failure of the bests
//! and course files.
pub mod bests;
pub mod course;
pub mod deleted;
pub mod facts;

pub(crate) const RETRY_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CardRefused;

pub(crate) trait CardError: Copy {
    fn media(self) -> Option<device_api::bulk::Error>;
}

macro_rules! card_error {
    ($error:ty) => {
        impl $crate::kept::CardError for $error {
            fn media(self) -> Option<device_api::bulk::Error> {
                use firmware_services::{files, volume};
                match self {
                    Self::Volume(volume::Error::Media(error))
                    | Self::Files(files::Error::Media(error)) => Some(error),
                    _ => None,
                }
            }
        }
    };
}
pub(crate) use card_error;

pub(crate) fn refusal(error: impl CardError) -> Result<(), CardRefused> {
    use device_api::bulk::Error::*;
    match error.media() {
        Some(Failed | Range) => Err(CardRefused),
        _ => Ok(()),
    }
}

/// Why a file on FILES could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No card, no FILES volume or no formatted file store: nothing to keep
    /// the file in.
    Unavailable,
    /// The medium failed this time; trying again may work.
    Media,
    /// A file is there but does not decode.
    Damaged,
    /// The file is of another version: kept, and never written over.
    Foreign,
    /// The store has no room for the file.
    Full,
}
