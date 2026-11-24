//! The minimal tile kit: every new screen is built from these tiles
//! and the containers row, column, stack and scroll, nothing bespoke.
//!
//! - A tile's setting byte names an entry of its spec table: an enum with a
//!   fixed id per entry and a `spec()` that maps it to what the tile draws,
//!   so a screen reads as data, `field(Field::Pwr)`. Each screen family owns
//!   a block of ids in each table (see the enum's doc); two entries with one
//!   id fail to compile.
//! - Colours come from the look in force, `data.theme()`, read only in
//!   `prepare`; gaps, radii and faces from `theme::LAYOUT`, which both looks
//!   share, so a layout solves once for both.
//! - A tile with an action takes the focus ring and the tap with no code of
//!   its own: `Tile::action` returns its spec's action.
mod band;
pub mod bar;
pub mod chart;
pub mod facts;
pub mod field;
pub mod item;
pub mod rule;
pub mod step;
pub mod symbol;
pub mod text;
pub mod top_line;

use super::node::{Node, Size};
use super::registry::{FIELD, RULE};
use crate::screens::scaled::Scaled;

/// A spec table: an enum whose ids are the setting bytes a tree stores,
/// each mapped to its spec, and the `Arg` that decodes it.
macro_rules! specs {
    (
        $(#[$meta:meta])*
        pub enum $name:ident: $spec:ty {
            $($variant:ident = $id:literal => $value:expr,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum $name {
            $($variant = $id,)*
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            pub const fn spec(self) -> &'static $spec {
                match self {
                    $(Self::$variant => &$value,)*
                }
            }
        }

        impl crate::compose::tile::Arg for $name {
            fn decode(raw: u8) -> Option<Self> {
                match raw {
                    $($id => Some(Self::$variant),)*
                    _ => None,
                }
            }

            fn encode(self) -> u8 {
                self as u8
            }

            fn label(self) -> &'static str {
                self.spec().label()
            }
        }
    };
}
pub(crate) use specs;

/// `n` of the layout's units on `at`'s panel.
pub(crate) const fn px(at: &Scaled, n: u8) -> usize {
    at.unit(n as usize)
}

/// A rule across a column or down a row. It takes its look's thickness, so
/// rules that share a line each keep a pixel where units would round away.
pub const fn rule() -> Node {
    Node::leaf(RULE, 0, Size::CONTENT)
}

/// A ride-grid field that shares its row's or column's room.
pub const fn field(field: field::Field) -> Node {
    Node::leaf(FIELD, field as u8, Size::grow(1))
}

/// A grid of `rows` of cells as one column that takes `size`: each row
/// shares the column's height, each cell its row's width, with a rule
/// between neighbours, so the rules line up down the whole grid.
pub fn grid<const C: usize>(rows: &[[Node; C]], size: Size) -> impl Iterator<Item = Node> + '_ {
    let count = rows.len() as u8;
    let head = Node::col(2 * count - 1, size);
    let body = rows.iter().enumerate().flat_map(|(at, cells)| {
        let between = (at > 0).then(rule);
        let line = Node::row(2 * C as u8 - 1, Size::grow(1));
        let cells = cells
            .iter()
            .enumerate()
            .flat_map(|(at, &cell)| (at > 0).then(rule).into_iter().chain([cell]));
        between.into_iter().chain([line]).chain(cells)
    });
    core::iter::once(head).chain(body)
}

#[cfg(test)]
mod tests;
