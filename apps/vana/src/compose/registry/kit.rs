//! Ids 112–123: the minimal kit every rebuilt screen draws with
//! (`compose::kit`).
use crate::compose::kit::{bar, facts, field, item, rule, symbol, text, top_line};
use crate::compose::tile::{Component, Erased};

pub const FIELD: u8 = 112;
pub const TOP_LINE: u8 = 113;
pub const RULE: u8 = 114;
pub const TEXT: u8 = 115;
pub const SYMBOL: u8 = 116;
pub const ITEM: u8 = 117;
pub const BAR: u8 = 118;
pub const FACTS: u8 = 119;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<field::FieldTile>::NEW,
    &Erased::<top_line::TopLine>::NEW,
    &Erased::<rule::Rule>::NEW,
    &Erased::<text::Text>::NEW,
    &Erased::<symbol::SymbolTile>::NEW,
    &Erased::<item::Item>::NEW,
    &Erased::<bar::Bar>::NEW,
    &Erased::<facts::Facts>::NEW,
];

const _: () = assert!(TILES.len() == (FACTS - FIELD) as usize + 1);
