//! What VANA keeps on the FILES volume beside the ride journals: the rider's
//! bests, each saved ride's facts and the rides the rider hid.
pub mod bests;
pub mod deleted;
pub mod facts;

pub(crate) const RETRY_MS: u64 = 5_000;

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
