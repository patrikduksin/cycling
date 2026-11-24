//! A chart: the box a page's chart painter fills row by row under the
//! tiles, such as the whole workout as one bar a step.
use super::{px, specs};
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Rect;
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;

pub struct Chart;

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    /// The fewest columns and rows the chart reads in.
    pub least: (u8, u8),
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A chart. Ids: Stats and Climb 0–63, Workout 64–127.
    pub enum Plot: Spec {
        Workout = 64 => Spec { name: "WORKOUT", least: (40, 20) },
    }
}

impl Tile for Chart {
    type Arg = Plot;
    const NAME: &'static str = "chart";

    fn forms(plot: Plot, at: &Scaled) -> Forms {
        let (columns, rows) = plot.spec().least;
        forms(&[Form {
            width: px(at, columns) as u16,
            height: px(at, rows) as u16,
            shapes: 0,
            runs: 0,
        }])
    }

    fn heavy() -> bool {
        true
    }

    fn prepare(_: Plot, _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}
}

/// Where a chart's painter draws in the chart's box: inside its gutters.
pub fn plotted(rect: Rect, at: &Scaled) -> Rect {
    let gutter = px(at, LAYOUT.space.gutter);
    Rect {
        left: rect.left + gutter,
        right: rect.right.saturating_sub(gutter).max(rect.left + gutter),
        ..rect
    }
}
