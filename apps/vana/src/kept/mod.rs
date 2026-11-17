//! What VANA keeps on the FILES volume beside the ride journals: the rider's
//! bests, each saved ride's facts and the rides the rider hid.
pub mod bests;
pub mod course;
pub mod deleted;
pub mod facts;

use crate::ride::medium::Medium;
use device_api::bulk::ReadWrite;

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

/// Old FILES records would attach to new rides that reuse the ids.
/// `#[inline(never)]` keeps the FILES buffers out of the callers' frames.
#[inline(never)]
pub(crate) fn forget_journal(
    facts: &mut facts::store::Kept,
    deleted: &mut deleted::Hidden,
    bulk: &mut impl ReadWrite,
    medium: Medium,
    now: u64,
) -> Result<(), CardRefused> {
    let history = crate::workout::history::forget(bulk, medium.journal()).or_else(|error| {
        log::warn!(target: "sdk.workout", "history_forget_failed error={}", error.name());
        refusal(error)
    });
    let facts = facts.forget(bulk, medium);
    let deleted = deleted.forget(bulk, medium, now);
    history.and(facts).and(deleted)
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
