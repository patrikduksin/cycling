//! The road ahead from an offline terrain pack, without a planned route.
//!
//! A pack (built by `tools/terrain`, format in `docs/terrain.md`) holds the
//! road graph of a region in fixed lat/lon tiles, with an elevation at every
//! vertex taken from a DEM on the host. This crate reads it through a small
//! [`Source`] (`read(offset, buf)`), keeps recently used tiles in a
//! caller-owned arena, matches GPS fixes to a road, follows the most likely
//! road ahead and returns its elevation profile.
//!
//! Everything is bounded: [`Lookahead::step`] does one unit of work (one
//! chunk of a tile read, one match or one prediction) and returns, so the
//! caller can interleave it with rendering. No allocation; integer pack
//! data, `f32` geometry.
//!
//! A map draws the cached tiles' roads through [`Lookahead::roads`], and
//! asks for the tiles its view needs with [`Lookahead::want`]; they load
//! only while the look-ahead has nothing of its own to do.
//!
//! What a climb is, and how it is shown, belongs to the application; this
//! crate only says where the road goes and how high it is.
#![no_std]

#[cfg(test)]
extern crate std;

mod cache;
mod engine;
mod geo;
mod matcher;
pub mod pack;
mod predict;
mod roads;
#[cfg(test)]
mod tests;
pub mod tile;

pub use cache::{Cache, MAX_TILE_BYTES, PAGE_BYTES};
pub use engine::{
    Ahead, BEHIND_SAMPLES, End, Fix, Lookahead, MAX_SAMPLES, SAMPLE_M, Status, WANTED,
};
pub use matcher::Matched;
pub use pack::{Error, Header, Source, TileKey};
pub use roads::{Road, RoadClass};
pub use tile::TileId;
