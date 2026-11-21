//! Every tile kind, by the id a tree stores. Ids come in one block per
//! family, each family in a file of its own, so adding a tile touches only
//! its family's file.
use super::tile::Component;

mod climb_workout;
mod primitive;
mod ride;
mod script;
mod scroll;
mod statics;

#[allow(unused_imports)]
pub use {climb_workout::*, primitive::*, ride::*, script::*, scroll::*, statics::*};

/// A family's block of ids and its tiles, the first at the block's start.
struct Family {
    first: u8,
    end: u8,
    tiles: &'static [&'static dyn Component],
}

const fn family(first: u8, end: u8, tiles: &'static [&'static dyn Component]) -> Family {
    Family { first, end, tiles }
}

/// One line a family, in id order.
const FAMILIES: &[Family] = &[
    family(0, 15, primitive::TILES),
    #[cfg(test)]
    family(
        TAP,
        TAP + 1,
        &[&super::tile::Erased::<super::input::tests::Tap>::NEW],
    ),
    family(16, 32, ride::TILES),
    family(32, 40, climb_workout::TILES),
    family(48, 64, statics::TILES),
    family(64, 80, scroll::TILES),
    family(80, 96, script::TILES),
];

const _: () = {
    let mut at = 0;
    while at < FAMILIES.len() {
        let family = &FAMILIES[at];
        assert!(family.tiles.len() <= (family.end - family.first) as usize);
        assert!(at == 0 || FAMILIES[at - 1].end <= family.first);
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
