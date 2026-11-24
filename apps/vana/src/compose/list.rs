//! A scroll's items: the page of equal cards its viewport shows, and the
//! renderer that draws the scroll's template once on each card in view.
//!
//! The template is solved once, one gap below the viewport's top and a
//! gutter in from either side, where the focus ring and the scroll bar
//! draw. Card `i` sits `i` pitches further down the page, a pitch being the
//! template's length and one gap, and the same gap opens above the first
//! card and below the last, so a list's gaps are even. A frame draws only
//! the cards in view, where they show in the viewport from canvas row
//! `PAGE` (`screens::scroll`), so however long the list no card's rows
//! leave the canvas.
use super::data::{Data, Edited, Effort, Entry, ListKind, Listed, Metric, OnGauge, Row};
use super::input::{Action, Focus, ListAct};
use super::node::What;
use super::parse::Tree;
use super::registry;
use super::solve::{Placed, Rect16, Scroll};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, CARD, PRESSED, RAISED, Rect, SECONDARY};
use crate::screens::scaled::Scaled;
use crate::screens::scroll::{Column, Layout, PAGE};

const GAP_UNITS: usize = 8;
const RADIUS_UNITS: usize = 12;
const RING_UNITS: usize = 3;
/// The scroll bar's columns, this far right of the cards, in the gutter.
const BAR_GAP_UNITS: usize = 4;
const BAR_UNITS: usize = 3;
/// Cards a viewport shows at once, whole or in part, at most: accept
/// refuses a scroll that shows more, so a frame keeps every one.
pub const MOST_IN_VIEW: usize = 8;
/// Tiles an item's template holds at most.
pub const MOST_TEMPLATE_TILES: usize = 8;

/// A template's tile: its rows on the card and what it costs the canvas.
#[derive(Clone, Copy, Debug)]
pub struct Costed {
    pub top: u16,
    pub bottom: u16,
    pub shapes: u8,
    pub runs: u8,
}

/// Between cards, and above the first and below the last.
pub fn gap(at: &Scaled) -> u16 {
    at.unit(GAP_UNITS) as u16
}

/// Either side of the cards, inside the scroll's box: the scroll bar's
/// room, which holds the focus ring's too.
pub fn gutter(at: &Scaled) -> u16 {
    (at.unit(BAR_GAP_UNITS) + at.unit(BAR_UNITS)) as u16
}

/// Where a scroll of `viewport` places its template before a tile of it is
/// placed, and where it stays when no tile of it is present: one gap down
/// and a gutter in, so every card it draws keeps clear of the box's edges.
pub fn template_at(viewport: Rect16, at: &Scaled) -> Rect16 {
    let (top, gutter) = (viewport.top + gap(at), gutter(at));
    Rect16 {
        left: viewport.left + gutter,
        top,
        right: viewport.right - gutter,
        bottom: top,
    }
}

/// The page of cards a scroll's `viewport` shows, its `template` placed on
/// the first.
pub fn page(viewport: Rect16, template: Rect16, at: &Scaled) -> Layout {
    let gap = usize::from(gap(at));
    let height = usize::from(template.height());
    Layout {
        top: usize::from(viewport.top),
        bottom: usize::from(viewport.bottom),
        column: Column {
            left: usize::from(template.left),
            right: usize::from(template.right),
            top: usize::from(template.top - viewport.top),
            height,
            pitch: height + gap,
        },
        end: gap,
        margin: gap,
        pin: None,
    }
}

/// The most cards that show at once, any of their rows.
pub fn in_view(viewport: Rect16, template: Rect16, at: &Scaled) -> usize {
    let pitch = usize::from(template.height() + gap(at));
    usize::from(viewport.height()) / pitch + 2
}

/// The most a scroll of `list` costs the canvas at any offset: each card
/// in view, its tiles whose rows the viewport shows, the focus ring and the
/// scroll bar, or what the list says while empty. `prepare` leaves out the
/// rest the same way, so a tall panel's cards fit the canvas beside what
/// stays put.
pub fn most(viewport: Rect16, template: Rect16, tiles: &[Costed], at: &Scaled) -> (u16, u16) {
    let (item, length) = (template.height(), viewport.height());
    let pitch = item + gap(at);
    let mut most = (0, 0);
    for start in 0..pitch {
        let end = start + length;
        // The focus ring round one card, and the scroll bar's track and
        // thumb.
        let (mut shapes, mut runs) = (1 + 2, 0);
        let mut top = 0;
        while top < end {
            if top + item > start {
                shapes += 1;
            }
            for tile in tiles {
                if top + tile.top < end && top + tile.bottom > start {
                    shapes += u16::from(tile.shapes);
                    runs += u16::from(tile.runs);
                }
            }
            top += pitch;
        }
        most = (most.0.max(shapes), most.1.max(runs));
    }
    most
}

/// Draw the card of each item in view, and on it the template's tiles for
/// that item.
pub fn prepare(
    tree: &Tree,
    scroll: &Scroll,
    template: &[Placed],
    data: &dyn Data,
    out: &mut Canvas,
) {
    let at = tree.at();
    let What::Scroll { list } = tree.nodes()[usize::from(scroll.node)].known() else {
        return;
    };
    let page = scroll.page(at);
    let listed = data.listed(list);
    let (count, offset) = (data.list(list), listed.offset);
    bar(&page, usize::from(count), offset, at, out);
    // The canvas rows the viewport shows: tiles wholly outside them are
    // left out.
    let shown = PAGE..PAGE + page.viewport();
    for index in page.visible(usize::from(count), offset) {
        let card = page.on_canvas(index, offset);
        let index = index as u16;
        let focused = listed.focus == Some(index);
        chrome(card, focused, listed.pressed == Some(index), at, out);
        // A card in view starts at most a card above `PAGE`, which lies
        // more than two panels down, so below the template's screen rows.
        let down = card.top as u16 - scroll.template.top;
        let item = Item {
            data,
            row: Row {
                list,
                index,
                focused,
            },
        };
        for placed in template {
            let What::Leaf { component, arg } = tree.nodes()[usize::from(placed.node)].known()
            else {
                continue;
            };
            let rect = placed.rect;
            let (top, bottom) = (rect.top + down, rect.bottom + down);
            if usize::from(top) >= shown.end || usize::from(bottom) <= shown.start {
                continue;
            }
            let placed = Placed {
                rect: Rect16 {
                    top,
                    bottom,
                    ..rect
                },
                ..*placed
            };
            registry::tile(component).prepare(arg, &placed, at, &item, out);
        }
    }
}

/// A list longer than its viewport: a track down the viewport in the
/// gutter at its right, the part in view lit. It stays put while the cards
/// scroll under it.
fn bar(page: &Layout, count: usize, offset: i32, at: &Scaled, out: &mut Canvas) {
    let Some((start, end)) = page.thumb(count, offset) else {
        return;
    };
    let left = page.column.right + at.unit(BAR_GAP_UNITS);
    for (top, bottom, color) in [(page.top, page.bottom, CARD), (start, end, SECONDARY)] {
        out.shape(Shape::Bar {
            rect: Rect {
                left,
                top: page.held(top),
                right: left + at.unit(BAR_UNITS),
                bottom: page.held(bottom),
            },
            radius: 1,
            color,
        });
    }
}

/// A card: raised and ringed while focused, darker under a finger.
fn chrome(card: Rect, focused: bool, pressed: bool, at: &Scaled, out: &mut Canvas) {
    let (radius, ring) = (at.unit(RADIUS_UNITS), at.unit(RING_UNITS));
    if focused {
        out.shape(Shape::Panel {
            rect: Rect {
                left: card.left - ring,
                top: card.top - ring,
                right: card.right + ring,
                bottom: card.bottom + ring,
            },
            radius: radius + ring,
            color: ACCENT,
        });
    }
    out.shape(Shape::Panel {
        rect: card,
        radius,
        color: if pressed {
            PRESSED
        } else if focused {
            RAISED
        } else {
            CARD
        },
    });
}

/// The card under a finger at (`x`, `y`) of a list of `count` scrolled to
/// `offset`: a tap focuses it and acts on it.
pub fn hit(
    scroll: &Scroll,
    at: &Scaled,
    count: u16,
    offset: i32,
    x: u16,
    y: u16,
) -> Option<(Focus, Action)> {
    let index =
        scroll
            .page(at)
            .card_at(usize::from(count), offset, usize::from(x), usize::from(y))?;
    Some((
        Focus::Tile(scroll.node),
        Action::List(ListAct::Tap(index as u16)),
    ))
}

/// What a template's tiles read: the frame's data, and the item they draw.
struct Item<'d> {
    data: &'d dyn Data,
    row: Row,
}

impl Data for Item<'_> {
    fn metric(&self, metric: Metric) -> Option<i32> {
        self.data.metric(metric)
    }

    fn zone(&self, effort: Effort) -> Option<u8> {
        self.data.zone(effort)
    }

    fn list(&self, list: ListKind) -> u16 {
        self.data.list(list)
    }

    fn gauge(&self, effort: Effort) -> Option<OnGauge> {
        self.data.gauge(effort)
    }

    fn target(&self, effort: Effort) -> Option<[OnGauge; 2]> {
        self.data.target(effort)
    }

    fn listed(&self, list: ListKind) -> Listed {
        self.data.listed(list)
    }

    fn entry(&self, list: ListKind, index: u16) -> Option<Entry<'_>> {
        self.data.entry(list, index)
    }

    fn row(&self) -> Option<Row> {
        Some(self.row)
    }

    fn heading(&self, list: ListKind) -> &str {
        self.data.heading(list)
    }

    fn saving(&self) -> Option<(&str, bool)> {
        self.data.saving()
    }

    fn edited(&self) -> Option<Edited<'_>> {
        self.data.edited()
    }

    fn sensors(&self) -> Option<&crate::screens::sensors::Menu> {
        self.data.sensors()
    }
}
