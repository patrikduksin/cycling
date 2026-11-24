//! Ids 112–123: the minimal tile kit (`compose::kit`).
use crate::compose::kit::{
    bar, chart, facts, field, item, picture, rule, stat, step, symbol, text, top_line,
};
use crate::compose::tile::{Component, Erased};

pub const FIELD: u8 = 112;
pub const TOP_LINE: u8 = 113;
pub const RULE: u8 = 114;
pub const TEXT: u8 = 115;
pub const SYMBOL: u8 = 116;
pub const ITEM: u8 = 117;
pub const BAR: u8 = 118;
pub const FACTS: u8 = 119;
pub const STEP: u8 = 120;
pub const CHART: u8 = 121;
pub const STAT: u8 = 122;
pub const PICTURE: u8 = 123;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<field::FieldTile>::NEW,
    &Erased::<top_line::TopLine>::NEW,
    &Erased::<rule::Rule>::NEW,
    &Erased::<text::Text>::NEW,
    &Erased::<symbol::SymbolTile>::NEW,
    &Erased::<item::Item>::NEW,
    &Erased::<bar::Bar>::NEW,
    &Erased::<facts::Facts>::NEW,
    &Erased::<step::StepTile>::NEW,
    &Erased::<chart::Chart>::NEW,
    &Erased::<stat::StatTile>::NEW,
    &Erased::<picture::PictureTile>::NEW,
];

const _: () = assert!(TILES.len() == (PICTURE - FIELD) as usize + 1);
