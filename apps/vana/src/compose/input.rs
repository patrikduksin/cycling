//! Input from the tree: which stop a button or a finger reaches, the cues
//! that say what each button does, and the route from an action to the
//! owner that carries it out.
//!
//! The focus ring is the screen's ACT button, its screen stop, when the
//! screen sets one, then the tiles that have an action, in preorder. A
//! screen that sets MOVE runs that action on every press and has no focus;
//! a screen that leaves MOVE free steps the ring with it, wrapping. ACT runs
//! the current stop. BACK is always the screen's.
use super::solve::{Placed, Rect16, Solved};
use crate::screens::cue::{Cue, Icon, Keys};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Page;
use device_api::input::{Role, Side};

/// What a press or a tap asks for, named by the owner that carries it out.
/// Each owner judges the request by its own state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Go(Go),
    /// A press on a ride flow page, which the ride turns into start, pause,
    /// resume, lap, end, save or discard by its state.
    Ride(Role),
    /// Lengthen the workout step by one extension.
    ExtendStep,
    /// Stop following the course.
    CourseOff,
    Page(OnPage),
    /// What the buttons or a finger do to the list shown, which its owner
    /// carries out on the item it keeps focused.
    List(ListAct),
    Sheet(SheetAct),
    /// Start recording from Home.
    Start(Plan),
}

/// What a ride started from Home plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Today's planned workout.
    Today,
    /// No workout, though one is planned.
    Free,
}

impl Action {
    /// Accept lets it go without a key path: the sheet opens only to a
    /// finger.
    pub const fn touch_only(self) -> bool {
        matches!(self, Self::Sheet(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetAct {
    Close,
    Dimmer,
    Brighter,
    Level,
    DayNight,
    Sensors,
    Phone,
}

/// A press or a tap on a list's items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListAct {
    /// MOVE: focus the next item, wrapping, and show it.
    Step,
    /// MOVE brought the ring to the list: focus its first item and show it.
    Enter,
    /// ACT: act on the focused item, once it shows whole.
    Act,
    /// A tap: focus this item and act on it.
    Tap(u16),
}

/// Where navigation goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Go {
    /// Open a page through its opener.
    Open(Page),
    /// The next page of the deck shown.
    Next,
    Map,
    /// Ask to turn off.
    TurnOff,
}

/// What the page or sheet shown does with its own things.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnPage {
    /// The item at this index on the page shown.
    Item(u8),
    /// Answer the question shown.
    Answer(bool),
    /// Leave the page; its owner knows where, and what it closes on the way.
    Back,
    /// Zero the power meter shown.
    Calibrate,
    /// Ask to forget the sensor shown.
    Forget,
    /// Search for sensors again.
    Search,
    Pair,
}

/// The owners an action reaches.
pub trait Owners {
    fn go(&mut self, go: Go);
    fn ride(&mut self, role: Role);
    fn extend_step(&mut self);
    fn course_off(&mut self);
    fn page(&mut self, act: OnPage);
    fn list(&mut self, act: ListAct);
    fn sheet(&mut self, act: SheetAct);
    fn start(&mut self, plan: Plan);
}

/// Hand `action` to the owner that carries it out.
pub fn apply(action: Action, owners: &mut impl Owners) {
    match action {
        Action::Go(go) => owners.go(go),
        Action::Ride(role) => owners.ride(role),
        Action::ExtendStep => owners.extend_step(),
        Action::CourseOff => owners.course_off(),
        Action::Page(act) => owners.page(act),
        Action::List(act) => owners.list(act),
        Action::Sheet(act) => owners.sheet(act),
        Action::Start(plan) => owners.start(plan),
    }
}

/// What one button does on a screen, and the icon of its cue, if it shows
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub action: Action,
    pub cue: Option<Icon>,
}

/// A screen's own buttons. ACT left `None` leaves the ring to the tiles;
/// MOVE left `None` steps the ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons {
    pub act: Option<Button>,
    pub back: Option<Button>,
    pub move_: Option<Button>,
    /// What holding MOVE for 1.5 s does, which accept counts as a way to a
    /// tile's action like a press.
    pub held: Option<Action>,
}

impl Buttons {
    pub const FREE: Self = Self {
        act: None,
        back: None,
        move_: None,
        held: None,
    };

    const fn of(&self, role: Role) -> Option<Button> {
        match role {
            Role::Act => self.act,
            Role::Back => self.back,
            Role::Move => self.move_,
        }
    }

    /// Whether a button of this screen does what `action` does. Paging on
    /// with MOVE reaches the map too.
    pub fn reach(&self, action: Action) -> bool {
        let pressed = [self.act, self.back, self.move_].into_iter().flatten();
        let next = self
            .move_
            .is_some_and(|button| button.action == Action::Go(Go::Next));
        pressed
            .map(|button| button.action)
            .chain(self.held)
            .any(|own| own == action)
            || (next && action == Action::Go(Go::Map))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controls {
    buttons: Buttons,
    cues: Option<Cues>,
}

impl Controls {
    /// No buttons and no cues: MOVE steps the tiles, ACT runs the focused
    /// one, BACK does nothing.
    pub const FREE: Self = Self {
        buttons: Buttons::FREE,
        cues: None,
    };

    pub fn new(buttons: Buttons, keys: Keys) -> Self {
        Self {
            buttons,
            cues: Some(cues(&buttons, keys)),
        }
    }

    pub const fn buttons(&self) -> &Buttons {
        &self.buttons
    }

    /// Every cue the buttons set: the tabs the tiles keep clear of.
    pub const fn cues(&self) -> Option<&Cues> {
        self.cues.as_ref()
    }

    /// The cues shown with `focus`: ACT's only while the focus is on the
    /// screen stop. Its tab stays clear either way.
    pub fn shown(&self, focus: Focus) -> impl Iterator<Item = Cue> + '_ {
        let cues = self.cues.as_ref().map_or(&[][..], Cues::shown);
        cues.iter()
            .copied()
            .filter(move |cue| cue.role != Role::Act || focus == Focus::Screen)
    }
}

/// The key tabs a screen shows, made only by [`cues`] from its buttons.
/// `Screen::cues` hands back the ones accept fitted its tiles clear of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cues {
    keys: Keys,
    shown: heapless::Vec<Cue, 3>,
}

impl Cues {
    pub fn shown(&self) -> &[Cue] {
        &self.shown
    }

    /// `area` short of the tabs on its top and bottom edges. Cutting a tile
    /// there instead would cost a content-height row the rows it needs.
    pub(super) fn within(&self, area: Rect16, at: &Scaled) -> Rect16 {
        self.shown.iter().fold(area, |area, cue| {
            let tab = cue.tab(&self.keys, at);
            match self.keys.of_role(cue.role).edge {
                Side::Bottom => Rect16 {
                    bottom: area.bottom.min(tab.top as u16),
                    ..area
                },
                Side::Top => Rect16 {
                    top: area.top.max(tab.bottom as u16),
                    ..area
                },
                Side::Left | Side::Right => area,
            }
        })
    }

    pub(super) fn clear(&self, rect: Rect16, at: &Scaled) -> Rect16 {
        let clear = self
            .shown
            .iter()
            .fold(rect.rect(), |rect, cue| cue.clear(&self.keys, at, rect));
        let clear = Rect16::of(clear);
        Rect16 {
            right: clear.right.max(clear.left),
            bottom: clear.bottom.max(clear.top),
            ..clear
        }
    }
}

pub fn cues(buttons: &Buttons, keys: Keys) -> Cues {
    let shown = [Role::Act, Role::Back, Role::Move]
        .into_iter()
        .filter_map(|role| Some(Cue::new(role, buttons.of(role)?.cue?)))
        .collect();
    Cues { keys, shown }
}

/// The current stop: the screen's ACT, or a tile kept by its node in the
/// tree so it survives a re-solve. The default is the stop the screen opens
/// on: the tile that asks for it (`Tile::opens`), else the first. A screen
/// with such a tile opens on it every time; any other keeps its focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Screen,
    Tile(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop<'s> {
    Screen(Button),
    Tile(&'s Placed),
}

/// The tiles focus visits after the screen's stop, in preorder.
fn stops<'s>(solved: &'s Solved<'_>) -> impl Iterator<Item = &'s Placed> {
    solved.focus_order()
}

/// The focused tile: the one `from` names or, once that is gone, the next
/// after it, wrapping.
fn tile_from<'s>(solved: &'s Solved<'_>, from: u8) -> Option<&'s Placed> {
    stops(solved)
        .find(|placed| placed.node >= from)
        .or_else(|| stops(solved).next())
}

/// The stop ACT runs: the screen's ACT where MOVE is the screen's, else the
/// ring's current stop; none when nothing acts.
pub fn focused<'s>(buttons: &Buttons, solved: &'s Solved<'_>, focus: Focus) -> Option<Stop<'s>> {
    if buttons.move_.is_some() {
        return buttons.act.map(Stop::Screen);
    }
    match (focus, buttons.act) {
        (Focus::Screen, Some(act)) => Some(Stop::Screen(act)),
        (Focus::Screen, None) => tile_from(solved, solved.opening().unwrap_or(0)).map(Stop::Tile),
        (Focus::Tile(node), _) => tile_from(solved, node).map(Stop::Tile),
    }
}

/// Where focus rests after an action ran: back on the screen stop when the
/// screen has one.
fn after_action(buttons: &Buttons, node: u8) -> Focus {
    if buttons.act.is_some() {
        Focus::Screen
    } else {
        Focus::Tile(node)
    }
}

/// What a press of `role` asks for. A MOVE the screen leaves free steps
/// `focus`; ACT on a tile leaves focus where it rests after an action.
pub fn press(
    role: Role,
    buttons: &Buttons,
    solved: &Solved<'_>,
    focus: &mut Focus,
) -> Option<Action> {
    match role {
        Role::Back => buttons.back.map(|button| button.action),
        Role::Move if buttons.move_.is_some() => buttons.move_.map(|button| button.action),
        Role::Move => {
            let current = match focused(buttons, solved, *focus)? {
                Stop::Screen(_) => None,
                Stop::Tile(current) => Some(current.node),
            };
            let next = match current {
                None => stops(solved).next(),
                Some(current) => stops(solved).find(|placed| placed.node > current),
            };
            let next = match next {
                Some(next) => next,
                None if buttons.act.is_some() => {
                    *focus = Focus::Screen;
                    return None;
                }
                None => stops(solved).next()?,
            };
            *focus = Focus::Tile(next.node);
            None
        }
        Role::Act => match focused(buttons, solved, *focus)? {
            Stop::Screen(act) => Some(act.action),
            Stop::Tile(placed) => {
                *focus = after_action(buttons, placed.node);
                solved.action(placed)
            }
        },
    }
}

/// The stop under a finger at (`x`, `y`): what a tap on it asks for, and
/// where focus rests after.
pub fn hit(buttons: &Buttons, solved: &Solved<'_>, x: u16, y: u16) -> Option<(Focus, Action)> {
    let placed = stops(solved).find(|placed| {
        let rect = placed.rect;
        (rect.left..rect.right).contains(&x) && (rect.top..rect.bottom).contains(&y)
    })?;
    Some((after_action(buttons, placed.node), solved.action(placed)?))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::compose::data::Data;
    use crate::compose::node::{Flags, Node, Size};
    use crate::compose::parse::Screen;
    use crate::compose::registry::{SPACE, TAP};
    use crate::compose::reject::Why;
    use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
    use crate::screens::canvas::Canvas;
    use crate::screens::scaled::BASE;
    use std::vec::Vec;

    pub struct Tap;

    #[derive(Clone, Copy)]
    pub struct Item(u8);

    impl Arg for Item {
        fn decode(raw: u8) -> Option<Self> {
            (raw < 4).then_some(Self(raw))
        }

        fn encode(self) -> u8 {
            self.0
        }
    }

    impl Tile for Tap {
        type Arg = Item;
        const NAME: &'static str = "tap";

        fn forms(_: Item, _: &Scaled) -> Forms {
            forms(&[Form {
                width: 40,
                height: 30,
                shapes: 0,
                runs: 0,
            }])
        }

        fn prepare(_: Item, _: &Placed, _: &Scaled, _: &dyn Data, _: &mut Canvas) {}

        fn action(Item(n): Item) -> Option<Action> {
            (n > 0).then_some(Action::Page(OnPage::Item(n)))
        }
    }

    fn tap(n: u8) -> Node {
        Node::leaf(TAP, n, Size::CONTENT)
    }

    fn screen_nodes() -> [Node; 8] {
        [
            Node::col(3, Size::grow(1)),
            Node::row(3, Size::CONTENT),
            tap(1),
            tap(0),
            tap(2),
            Node::leaf(SPACE, 0, Size::grow(1)),
            Node::row(1, Size::CONTENT),
            tap(3),
        ]
    }

    fn screen() -> Screen {
        let bytes: Vec<u8> = screen_nodes()
            .iter()
            .flat_map(|node| node.bytes())
            .collect();
        Screen::parse(0, &bytes, &BASE, Controls::FREE).unwrap()
    }

    fn item(n: u8) -> Option<Action> {
        Some(Action::Page(OnPage::Item(n)))
    }

    fn solved(screen: &Screen) -> Solved<'_> {
        let mut solved = Solved::EMPTY;
        screen.solve(Flags::NONE, &mut solved);
        solved
    }

    fn ring(buttons: &Buttons, solved: &Solved, count: usize) -> Vec<Option<Action>> {
        let mut focus = Focus::default();
        (0..count)
            .map(|_| {
                let mut probe = focus;
                let acted = press(Role::Act, buttons, solved, &mut probe);
                assert_eq!(press(Role::Move, buttons, solved, &mut focus), None);
                acted
            })
            .collect()
    }

    const START: Button = Button {
        action: Action::Ride(Role::Act),
        cue: Some(Icon::Play),
    };
    const HOME: Button = Button {
        action: Action::Go(Go::Open(Page::Home)),
        cue: None,
    };
    const NEXT: Button = Button {
        action: Action::Go(Go::Next),
        cue: None,
    };
    const ACT: Buttons = Buttons {
        act: Some(START),
        ..Buttons::FREE
    };
    const DECK: Buttons = Buttons {
        act: Some(START),
        back: Some(HOME),
        move_: Some(NEXT),
        held: Some(Action::ExtendStep),
    };

    #[test]
    fn with_act_free_move_steps_the_tiles_that_act_in_preorder_and_wraps() {
        let screen = screen();
        let solved = solved(&screen);
        assert_eq!(
            ring(&Buttons::FREE, &solved, 7),
            [
                item(1),
                item(2),
                item(3),
                item(1),
                item(2),
                item(3),
                item(1)
            ]
        );
    }

    #[test]
    fn a_screen_act_is_the_first_stop_and_the_ring_wraps_back_to_it() {
        let screen = screen();
        let solved = solved(&screen);
        let start = Some(START.action);
        assert_eq!(
            ring(&ACT, &solved, 9),
            [
                start,
                item(1),
                item(2),
                item(3),
                start,
                item(1),
                item(2),
                item(3),
                start
            ]
        );
    }

    #[test]
    fn after_a_tile_acts_focus_returns_to_the_screen_stop() {
        let screen = screen();
        let solved = solved(&screen);
        let mut focus = Focus::default();
        assert_eq!(focus, Focus::Screen, "a screen opens on its first stop");
        press(Role::Move, &ACT, &solved, &mut focus);
        press(Role::Move, &ACT, &solved, &mut focus);
        assert_eq!(press(Role::Act, &ACT, &solved, &mut focus), item(2));
        assert_eq!(focus, Focus::Screen);
        let mut free = Focus::default();
        press(Role::Move, &Buttons::FREE, &solved, &mut free);
        assert_eq!(
            press(Role::Act, &Buttons::FREE, &solved, &mut free),
            item(2)
        );
        assert_eq!(free, Focus::Tile(4), "without a screen stop focus stays");
    }

    #[test]
    fn a_screen_that_keeps_move_runs_it_on_every_press_and_has_no_focus() {
        let screen = screen();
        let solved = solved(&screen);
        let mut focus = Focus::default();
        for _ in 0..5 {
            assert_eq!(
                press(Role::Move, &DECK, &solved, &mut focus),
                Some(NEXT.action)
            );
            assert_eq!(
                press(Role::Act, &DECK, &solved, &mut focus),
                Some(START.action)
            );
        }
        assert_eq!(focus, Focus::Screen);
        let paging = Buttons {
            move_: Some(NEXT),
            ..Buttons::FREE
        };
        assert_eq!(press(Role::Act, &paging, &solved, &mut focus), None);
        assert_eq!(focused(&paging, &solved, focus), None);
    }

    #[test]
    fn back_is_always_the_screens_and_leaves_the_focus() {
        let screen = screen();
        let solved = solved(&screen);
        let back = Buttons {
            back: Some(HOME),
            ..Buttons::FREE
        };
        let mut focus = Focus::Tile(4);
        assert_eq!(
            press(Role::Back, &back, &solved, &mut focus),
            Some(HOME.action)
        );
        assert_eq!(press(Role::Back, &Buttons::FREE, &solved, &mut focus), None);
        assert_eq!(focus, Focus::Tile(4));
    }

    #[test]
    fn a_screen_without_tiles_that_act_answers_only_its_own_buttons() {
        let nodes = [Node::leaf(SPACE, 0, Size::grow(1))];
        let bytes: Vec<u8> = nodes.iter().flat_map(|node| node.bytes()).collect();
        let screen = Screen::parse(0, &bytes, &BASE, Controls::FREE).unwrap();
        let solved = solved(&screen);
        let mut focus = Focus::default();
        for role in [Role::Act, Role::Back, Role::Move] {
            assert_eq!(press(role, &Buttons::FREE, &solved, &mut focus), None);
        }
        assert_eq!(
            press(Role::Act, &ACT, &solved, &mut focus),
            Some(START.action)
        );
        assert_eq!(press(Role::Move, &ACT, &solved, &mut focus), None);
        assert_eq!(focus, Focus::Screen, "the ring holds only the screen stop");
        assert_eq!(hit(&ACT, &solved, 10, 10), None);
    }

    #[test]
    fn a_finger_on_a_tile_that_acts_hits_it_and_anywhere_else_hits_nothing() {
        let screen = screen();
        let solved = solved(&screen);
        let acting: Vec<(Rect16, Option<Action>)> = solved
            .leaves()
            .iter()
            .map(|placed| (placed.rect, solved.action(placed)))
            .filter(|(_, action)| action.is_some())
            .collect();
        assert_eq!(acting.len(), 3);
        let (mut hits, mut misses) = (0, 0);
        let panel = BASE.panel;
        for y in 0..panel.height as u16 {
            for x in 0..panel.width as u16 {
                let under = acting.iter().find(|(rect, _)| {
                    (rect.left..rect.right).contains(&x) && (rect.top..rect.bottom).contains(&y)
                });
                let found = hit(&Buttons::FREE, &solved, x, y);
                assert_eq!(found.map(|(_, action)| action), under.and_then(|it| it.1));
                if let Some((mut focus, action)) = found {
                    let acted = press(Role::Act, &Buttons::FREE, &solved, &mut focus);
                    assert_eq!(acted, Some(action), "a tap focuses its tile");
                    let (focus, _) = hit(&ACT, &solved, x, y).unwrap();
                    assert_eq!(focus, Focus::Screen, "then the screen stop");
                    hits += 1;
                } else {
                    misses += 1;
                }
            }
        }
        assert_eq!(hits, 3 * 40 * 30);
        assert!(misses > 0);
    }

    #[test]
    fn cues_show_beside_the_buttons_that_set_an_icon() {
        let keys = crate::screens::C606.keys;
        assert!(cues(&Buttons::FREE, keys).shown().is_empty());
        let quiet = Buttons {
            back: Some(HOME),
            move_: Some(NEXT),
            ..Buttons::FREE
        };
        assert!(cues(&quiet, keys).shown().is_empty());
        let paused = Buttons {
            act: Some(START),
            back: Some(Button {
                action: Action::Ride(Role::Back),
                cue: Some(Icon::Stop),
            }),
            ..quiet
        };
        assert_eq!(
            cues(&paused, keys).shown(),
            [
                Cue::new(Role::Act, Icon::Play),
                Cue::new(Role::Back, Icon::Stop)
            ]
        );
    }

    #[test]
    fn the_act_cue_shows_only_while_focus_is_on_the_screen_stop() {
        let controls = Controls::new(ACT, crate::screens::C606.keys);
        let play = Cue::new(Role::Act, Icon::Play);
        assert_eq!(controls.shown(Focus::Screen).collect::<Vec<_>>(), [play]);
        assert_eq!(controls.shown(Focus::Tile(2)).count(), 0);
        assert_eq!(
            controls.cues().unwrap().shown(),
            [play],
            "its tab stays clear"
        );
    }

    #[test]
    fn where_the_screen_keeps_move_a_tile_must_do_what_a_button_does() {
        let refused = |buttons: Buttons| {
            let bytes: Vec<u8> = screen_nodes()
                .iter()
                .flat_map(|node| node.bytes())
                .collect();
            let controls = Controls::new(buttons, crate::screens::C606.keys);
            Screen::parse(0, &bytes, &BASE, controls)
                .err()
                .map(|reject| reject.why)
        };
        assert_eq!(refused(ACT), None, "MOVE free steps to every tile");
        assert_eq!(refused(DECK), Some(Why::NoButtonPath { node: 2 }));
        let item = |n| Button {
            action: Action::Page(OnPage::Item(n)),
            cue: None,
        };
        let reaching = Buttons {
            act: Some(item(1)),
            back: Some(item(2)),
            move_: Some(NEXT),
            held: Some(Action::Page(OnPage::Item(3))),
        };
        assert_eq!(refused(reaching), None);
        assert!(DECK.reach(Action::Go(Go::Map)), "MOVE pages toward the map");
        assert!(!ACT.reach(Action::Go(Go::Map)));
    }

    #[test]
    fn a_screen_accepted_with_cues_keeps_its_tiles_clear_of_them() {
        let device = &crate::screens::C606;
        let controls = Controls::new(ACT, device.keys);
        let nodes = [
            Node::col(2, Size::grow(1)),
            Node::leaf(SPACE, 0, Size::grow(1)),
            Node::row(2, Size::CONTENT),
            Node::leaf(SPACE, 0, Size::grow(1)),
            Node::leaf(TAP, 1, Size::grow(1)),
        ];
        let bytes: Vec<u8> = nodes.iter().flat_map(|node| node.bytes()).collect();
        let bare = Screen::parse(0, &bytes, &BASE, Controls::FREE).unwrap();
        let screen = Screen::parse(0, &bytes, &BASE, controls.clone()).unwrap();
        assert_eq!(screen.cues(), controls.cues());
        let (under, clear) = (solved(&bare), solved(&screen));
        let cue = controls.cues().unwrap().shown()[0];
        let tap = |solved: &Solved| {
            let placed = solved.leaves().iter().find(|placed| placed.node == 4);
            placed.unwrap().rect.rect()
        };
        assert!(
            cue.covers(tap(&under), device),
            "the tile sits on the cue's tab"
        );
        assert!(!cue.covers(tap(&clear), device));
    }

    #[derive(Default)]
    struct Reached(Vec<(&'static str, Action)>);

    impl Owners for Reached {
        fn go(&mut self, go: Go) {
            self.0.push(("navigation", Action::Go(go)));
        }

        fn ride(&mut self, role: Role) {
            self.0.push(("ride", Action::Ride(role)));
        }

        fn extend_step(&mut self) {
            self.0.push(("workout", Action::ExtendStep));
        }

        fn course_off(&mut self) {
            self.0.push(("course", Action::CourseOff));
        }

        fn page(&mut self, act: OnPage) {
            self.0.push(("page", Action::Page(act)));
        }

        fn list(&mut self, act: ListAct) {
            self.0.push(("list", Action::List(act)));
        }

        fn sheet(&mut self, act: SheetAct) {
            self.0.push(("sheet", Action::Sheet(act)));
        }

        fn start(&mut self, plan: Plan) {
            self.0.push(("ride", Action::Start(plan)));
        }
    }

    #[test]
    fn apply_hands_each_action_to_its_owner() {
        let every = [
            ("navigation", Action::Go(Go::Open(Page::Sensors))),
            ("navigation", Action::Go(Go::Next)),
            ("navigation", Action::Go(Go::Map)),
            ("navigation", Action::Go(Go::TurnOff)),
            ("ride", Action::Ride(Role::Act)),
            ("ride", Action::Ride(Role::Back)),
            ("ride", Action::Ride(Role::Move)),
            ("workout", Action::ExtendStep),
            ("course", Action::CourseOff),
            ("page", Action::Page(OnPage::Item(2))),
            ("page", Action::Page(OnPage::Answer(true))),
            ("page", Action::Page(OnPage::Answer(false))),
            ("page", Action::Page(OnPage::Back)),
            ("page", Action::Page(OnPage::Calibrate)),
            ("page", Action::Page(OnPage::Forget)),
            ("page", Action::Page(OnPage::Search)),
            ("sheet", Action::Sheet(SheetAct::Close)),
            ("sheet", Action::Sheet(SheetAct::Dimmer)),
            ("sheet", Action::Sheet(SheetAct::Brighter)),
            ("sheet", Action::Sheet(SheetAct::Level)),
            ("sheet", Action::Sheet(SheetAct::DayNight)),
            ("sheet", Action::Sheet(SheetAct::Sensors)),
            ("ride", Action::Start(Plan::Today)),
            ("ride", Action::Start(Plan::Free)),
        ];
        let mut reached = Reached::default();
        for (_, action) in every {
            apply(action, &mut reached);
        }
        assert_eq!(reached.0, every);
    }
}
