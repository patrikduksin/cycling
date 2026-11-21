//! A scroll an accepted tree holds draws inside its box at any length and
//! any offset, and MOVE reaches every stop past it.
use super::{bytes, panels, tile};
use crate::compose::builtin::{Choices, Laid, ScreenId};
use crate::compose::data::{Data, Effort, Entry, ListKind, Listed, Metric, Nothing};
use crate::compose::input::{self, Action, Controls, Cursor, Focus, ListAct};
use crate::compose::node::{Flags, Node, Size};
use crate::compose::parse::Screen;
use crate::compose::registry::{ENTRY, HEADING, PAD, SPACE};
use crate::compose::reject::{Misfit, Why};
use crate::compose::solve::Solved;
use crate::compose::tiles::entry::Line;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Rect;
use crate::screens::chrome::layout::text;
use crate::screens::scaled::Scaled;
use crate::screens::scroll::Layout;
use device_api::input::Role;
use std::string::String;
use std::vec::Vec;

/// `count` rides, scrolled to `offset`, the focus on `focus`.
struct Rows {
    count: u16,
    offset: i32,
    focus: Option<u16>,
}

impl Data for Rows {
    fn metric(&self, _: Metric) -> Option<i32> {
        None
    }

    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }

    fn list(&self, _: ListKind) -> u16 {
        self.count
    }

    fn listed(&self, _: ListKind) -> Listed {
        Listed {
            offset: self.offset,
            focus: self.focus,
            pressed: None,
        }
    }

    fn entry(&self, _: ListKind, _: u16) -> Option<Entry<'_>> {
        Some(Entry {
            title: "RIDE",
            detail: "DETAIL",
            note: "",
            note_color: 0,
            note_first: false,
            chosen: false,
        })
    }
}

fn parse(nodes: &[Node], at: &Scaled) -> Screen {
    Screen::parse(0, &bytes(nodes), at, Controls::FREE).unwrap()
}

fn solved(screen: &Screen) -> Solved<'_> {
    let mut solved = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut solved);
    solved
}

/// The canvas rows a viewport of `page` shows.
fn shown(page: &Layout) -> core::ops::Range<usize> {
    let top = page.canvas_row(page.top, 0).unwrap();
    top..top + page.viewport()
}

/// Every text the canvas inks, and where.
fn texts(canvas: &Canvas) -> Vec<(String, Rect)> {
    canvas.runs.iter().filter_map(text).collect()
}

fn inside(outer: Rect, inner: Rect) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

/// A screen that is one scroll, of cards of two lines.
fn full_width() -> [Node; 4] {
    [
        Node::scroll(ListKind::Rides, Size::grow(1)),
        Node::col(2, Size::units(90)),
        tile(ENTRY, Line::Title as u8, Size::CONTENT),
        tile(ENTRY, Line::Detail as u8, Size::CONTENT),
    ]
}

#[test]
fn a_long_history_draws_every_card_in_view_at_every_offset() {
    for at in panels() {
        let device = crate::screens::Device::of(at, &crate::screens::C606_BUTTONS);
        let laid = Laid::new(
            ScreenId::History,
            &device,
            Choices::default(),
            Flags::NONE,
            None,
        )
        .unwrap();
        let page = *laid.page().unwrap();
        let count = 2000;
        let end = (page.content(count) - page.viewport()) as i32;
        let pitch = page.column.pitch as i32;
        let offsets = (0..end).step_by(pitch as usize * 7 + 5).chain([end]);
        for offset in offsets {
            let data = Rows {
                count: count as u16,
                offset,
                focus: Some(0),
            };
            let mut canvas = Canvas::default();
            laid.prepare(&data, &mut canvas);
            canvas.assert_kept("2,000 rides");
            let texts = texts(&canvas);
            let whole: Vec<usize> = page
                .visible(count, offset)
                .filter(|&index| page.whole(index, offset))
                .collect();
            assert!(!whole.is_empty(), "{:?} at {offset}", at.panel);
            for index in whole {
                let card = page.on_canvas(index, offset);
                for line in ["RIDE", "DETAIL"] {
                    assert!(
                        texts
                            .iter()
                            .any(|(name, ink)| name == line && inside(card, *ink)),
                        "{:?}: card {index} at {offset} has no {line}",
                        at.panel
                    );
                }
            }
        }
    }
}

#[test]
fn a_scroll_as_wide_as_the_panel_rings_and_bars_inside_its_box() {
    for at in panels() {
        let screen = parse(&full_width(), &at);
        let solved = solved(&screen);
        let (scroll, _) = solved.scroll().unwrap();
        let viewport = scroll.viewport.rect();
        for focus in [0, 9] {
            let data = Rows {
                count: 10,
                offset: 0,
                focus: Some(focus),
            };
            let mut canvas = Canvas::default();
            solved.prepare(&data, &mut canvas);
            for shape in &canvas.shapes {
                let bounds = shape.bounds();
                assert!(
                    bounds.left >= viewport.left && bounds.right <= viewport.right,
                    "{:?}: {bounds:?} leaves {viewport:?}",
                    at.panel
                );
            }
        }
    }
}

#[test]
fn a_scroll_too_short_for_one_item_and_its_gaps_is_refused_at_accept() {
    for at in panels() {
        let nodes = [
            Node::col(2, Size::grow(1)),
            Node::scroll(ListKind::Workouts, Size::units(1)),
            tile(SPACE, 0, Size::units(1)),
            tile(SPACE, 0, Size::grow(1)),
        ];
        let reject = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).unwrap_err();
        assert!(
            matches!(reject.why, Why::Misfit(Misfit { node: 1, .. })),
            "{:?}: {reject}",
            at.panel
        );
    }
}

#[test]
fn an_empty_list_s_words_count_against_the_canvas_at_accept() {
    for at in panels() {
        let mut nodes = std::vec![Node::col(15, Size::grow(1))];
        let heading = tile(HEADING, ListKind::Workouts as u8, Size::CONTENT);
        nodes.extend(core::iter::repeat_n(heading, 14));
        nodes.push(Node::scroll(ListKind::Rides, Size::units(80)));
        nodes.push(tile(SPACE, 0, Size::units(12)));
        let reject = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).unwrap_err();
        assert!(
            matches!(reject.why, Why::Canvas { .. }),
            "{:?}: {reject}",
            at.panel
        );
    }
}

#[test]
fn an_empty_list_s_words_show_inside_its_viewport_or_it_is_refused() {
    for at in panels() {
        let short = [
            Node::col(2, Size::grow(1)),
            Node::scroll(ListKind::Rides, Size::units(24)),
            tile(SPACE, 0, Size::units(8)),
            tile(SPACE, 0, Size::grow(1)),
        ];
        let reject = Screen::parse(0, &bytes(&short), &at, Controls::FREE).unwrap_err();
        assert!(
            matches!(reject.why, Why::Misfit(Misfit { node: 1, .. })),
            "{:?}: {reject}",
            at.panel
        );
        let device = crate::screens::Device::of(at, &crate::screens::C606_BUTTONS);
        let laid = Laid::new(
            ScreenId::History,
            &device,
            Choices::default(),
            Flags::NONE,
            None,
        )
        .unwrap();
        let page = *laid.page().unwrap();
        let mut canvas = Canvas::default();
        laid.prepare(&Nothing, &mut canvas);
        let rows = shown(&page);
        for line in ["NO RIDES YET", "SAVED RIDES SHOW HERE"] {
            let (_, ink) = texts(&canvas)
                .into_iter()
                .find(|(name, _)| name == line)
                .unwrap();
            assert!(
                rows.contains(&ink.top) && ink.bottom <= rows.end,
                "{:?}: {line} at {ink:?} outside {rows:?}",
                at.panel
            );
        }
    }
}

#[test]
fn move_past_a_list_s_last_item_reaches_the_next_stop_and_the_ring_wraps() {
    for at in panels() {
        let nodes = [
            Node::col(2, Size::grow(1)),
            Node::scroll(ListKind::Rides, Size::grow(1)),
            tile(SPACE, 0, Size::units(90)),
            tile(PAD, 0, Size::CONTENT),
        ];
        let screen = parse(&nodes, &at);
        let solved = solved(&screen);
        let buttons = screen.controls().buttons();
        let move_ =
            |focus: &mut Focus, cursor| input::press(Role::Move, buttons, &solved, focus, cursor);
        let mut focus = Focus::default();
        let step = Some(Action::List(ListAct::Step));
        assert_eq!(move_(&mut focus, Cursor::Within), step, "{:?}", at.panel);
        assert_eq!(focus, Focus::Tile(1));
        assert_eq!(move_(&mut focus, Cursor::Last), None);
        assert_eq!(focus, Focus::Tile(3), "past the last item, the pad");
        assert_eq!(
            input::press(Role::Act, buttons, &solved, &mut focus, Cursor::Last),
            Some(Action::Page(input::OnPage::Lower))
        );
        assert_eq!(
            move_(&mut focus, Cursor::Last),
            Some(Action::List(ListAct::Enter)),
            "the ring wraps to the list, which starts again from its first item"
        );
        assert_eq!(focus, Focus::Tile(1));
    }
}

#[test]
fn move_on_a_list_that_is_the_only_stop_wraps_its_own_items() {
    for at in panels() {
        let screen = parse(&full_width(), &at);
        let solved = solved(&screen);
        let buttons = screen.controls().buttons();
        let mut focus = Focus::default();
        for cursor in [Cursor::Within, Cursor::Last] {
            assert_eq!(
                input::press(Role::Move, buttons, &solved, &mut focus, cursor),
                Some(Action::List(ListAct::Step))
            );
            assert_eq!(focus, Focus::Tile(0));
        }
    }
}

#[test]
fn a_stored_screen_with_a_scroll_draws_a_card_per_item() {
    for at in panels() {
        let screen = parse(&full_width(), &at);
        let solved = solved(&screen);
        let page = solved.scroll().unwrap().0.page(&at);
        let data = Rows {
            count: 2,
            offset: 0,
            focus: None,
        };
        let mut canvas = Canvas::default();
        solved.prepare(&data, &mut canvas);
        let texts = texts(&canvas);
        for index in 0..2 {
            let card = page.on_canvas(index, 0);
            let on = |line| {
                texts
                    .iter()
                    .filter(|(name, ink)| name == line && inside(card, *ink))
                    .count()
            };
            assert_eq!((on("RIDE"), on("DETAIL")), (1, 1), "{:?}", at.panel);
        }
        assert_eq!(texts.len(), 4, "{:?}: the template once a card", at.panel);
    }
}

#[test]
fn a_scroll_whose_template_a_flag_hides_draws_its_cards_inside_its_box() {
    use crate::compose::node::When;
    for at in panels() {
        let nodes = [
            Node::col(2, Size::grow(1)),
            Node::scroll(ListKind::Workouts, Size::units(24)),
            tile(SPACE, 0, Size::units(8)).when(When::new(Flags::RADAR, Flags::NONE)),
            tile(SPACE, 0, Size::grow(1)),
        ];
        let screen = parse(&nodes, &at);
        let solved = solved(&screen);
        let viewport = solved.scroll().unwrap().0.viewport.rect();
        let data = Rows {
            count: 10,
            offset: 0,
            focus: Some(1),
        };
        let mut canvas = Canvas::default();
        solved.prepare(&data, &mut canvas);
        for shape in &canvas.shapes {
            let bounds = shape.bounds();
            assert!(
                bounds.left >= viewport.left && bounds.right <= viewport.right,
                "{:?}: {bounds:?} leaves {viewport:?}",
                at.panel
            );
        }
    }
}

#[test]
fn an_empty_list_s_words_are_measured_in_the_width_they_draw_in() {
    use crate::compose::list::gutter;
    use firmware_shell::rendering::font::FaceRole;
    // One unit a pixel, so a spacer of units leaves the scroll exact pixels.
    let at = crate::screens::scaled::BASE;
    let words = at
        .face(FaceRole::Caption)
        .fit_width(b"SAVED RIDES SHOW HERE")
        .max(at.face(FaceRole::Body).fit_width(b"NO RIDES YET"));
    let scroll_of = |width: usize| {
        [
            Node::row(2, Size::grow(1)),
            Node::col(1, Size::grow(1)),
            Node::scroll(ListKind::Rides, Size::grow(1)),
            tile(SPACE, 0, Size::units(80)),
            tile(SPACE, 0, Size::units((at.panel.width - width) as u8)),
        ]
    };
    // The words fill the box but not the width between its gutters.
    let narrow = Screen::parse(0, &bytes(&scroll_of(words)), &at, Controls::FREE).unwrap_err();
    assert!(matches!(narrow.why, Why::Misfit(_)), "{narrow}");
    let screen = parse(&scroll_of(words + 2 * usize::from(gutter(&at))), &at);
    let mut canvas = Canvas::default();
    solved(&screen).prepare(&Nothing, &mut canvas);
    let texts: Vec<String> = texts(&canvas).into_iter().map(|(text, _)| text).collect();
    for line in ["NO RIDES YET", "SAVED RIDES SHOW HERE"] {
        assert!(texts.iter().any(|text| text == line), "{texts:?}");
    }
}
