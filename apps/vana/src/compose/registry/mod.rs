//! Every tile kind, by the id a tree stores. Ids come in one block per
//! family, each family in a file of its own, so adding a tile touches only
//! its family's file.
use super::tile::Component;

mod primitive;
mod ride;
mod script;
mod scroll;
mod statics;

#[allow(unused_imports)]
pub use {primitive::*, ride::*, script::*, scroll::*, statics::*};

/// A family's block of ids and its tiles, the first at the block's start.
struct Family {
    first: u8,
    end: u8,
    tiles: &'static [&'static dyn Component],
}

const FAMILIES: &[Family] = &[
    Family {
        first: 0,
        end: 15,
        tiles: primitive::TILES,
    },
    #[cfg(test)]
    Family {
        first: TAP,
        end: TAP + 1,
        tiles: &[&super::tile::Erased::<super::input::tests::Tap>::NEW],
    },
    Family {
        first: 16,
        end: 48,
        tiles: ride::TILES,
    },
    Family {
        first: 48,
        end: 64,
        tiles: statics::TILES,
    },
    Family {
        first: 64,
        end: 80,
        tiles: scroll::TILES,
    },
    Family {
        first: 80,
        end: 96,
        tiles: script::TILES,
    },
];

const _: () = {
    let mut at = 0;
    while at < FAMILIES.len() {
        let family = &FAMILIES[at];
        assert!(family.tiles.len() <= (family.end - family.first) as usize);
        at += 1;
    }
};

/// Kept for the input tests' tile, past the primitives.
#[cfg(test)]
pub const TAP: u8 = 15;

/// The tile kind `id` names, if there is one.
pub fn get(id: u8) -> Option<&'static dyn Component> {
    let family = FAMILIES
        .iter()
        .find(|family| (family.first..family.end).contains(&id))?;
    family.tiles.get(usize::from(id - family.first)).copied()
}

/// The tile kind of an id a parsed tree holds.
pub fn tile(id: u8) -> &'static dyn Component {
    match get(id) {
        Some(tile) => tile,
        None => unreachable!("parse admits only known kinds"),
    }
}

/// Every tile kind with its id.
pub fn every() -> impl Iterator<Item = (u8, &'static dyn Component)> {
    FAMILIES
        .iter()
        .flat_map(|family| (family.first..).zip(family.tiles.iter().copied()))
}
