//! Values as a replay reads them back from a record body: owned, so that
//! a server can hand them to VANA after the trace's bytes are gone.
use super::AntCapabilities;
use super::wire::{Reader, Wire};
use device_api::{ant, sound};
use std::sync::{Mutex, PoisonError};
use std::vec::Vec;

pub trait Replayed: Sized {
    fn replayed(r: &mut Reader<'_>) -> Self;
}

impl<T: for<'a> Wire<'a>> Replayed for T {
    fn replayed(r: &mut Reader<'_>) -> Self {
        T::take(r)
    }
}

/// `list` for the rest of the process: a device hands these out `'static`,
/// and a trace holds few distinct ones.
fn interned<T: PartialEq + Send + Sync + 'static>(
    known: &Mutex<Vec<&'static [T]>>,
    list: Vec<T>,
) -> &'static [T] {
    let mut known = known.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&kept) = known.iter().find(|kept| **kept == list.as_slice()) {
        return kept;
    }
    let kept: &'static [T] = Vec::leak(list);
    known.push(kept);
    kept
}

impl Replayed for ant::Capabilities {
    fn replayed(r: &mut Reader<'_>) -> Self {
        static KNOWN: Mutex<Vec<&'static [u8]>> = Mutex::new(Vec::new());
        let recorded = AntCapabilities::take(r);
        recorded.with_types(interned(&KNOWN, recorded.supported_types.to_vec()))
    }
}

impl Replayed for &'static [sound::Pattern] {
    fn replayed(r: &mut Reader<'_>) -> Self {
        static KNOWN: Mutex<Vec<&'static [sound::Pattern]>> = Mutex::new(Vec::new());
        let len = usize::take(r);
        let patterns = (0..len)
            .map_while(|_| r.read().map(|_| sound::Pattern::take(r)))
            .collect();
        interned(&KNOWN, patterns)
    }
}
