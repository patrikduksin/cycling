//! Touch routing, after UIKit's: hit-testing, gesture recognizers that see a
//! touch before the control under it, and a scroll view that holds a
//! touch back until it knows the finger will not scroll.
//!
//! - Hit-testing: when a finger lands, the screen names what lies under it
//!   ([`Hit`]): the control (a card, tile, row or pad), the list it sits in
//!   and whether the page takes swipes. The contact stays bound to that hit
//!   until it lifts, wherever the finger goes (UIKit's `UITouch.view`).
//! - Arbitration: the list's pan, the page's swipe and the control's tap
//!   compete for the contact, and one wins. Ten pixels of mostly vertical
//!   travel give it to the list, as much mostly sideways travel to the page's
//!   swipe, and leaving the tap slop any other way loses it for all. The
//!   control that tracked it is told it is cancelled, never activated
//!   (`cancelsTouchesInView`).
//! - The scroll view's delay: a control in a list shows as pressed only once
//!   the finger has rested `PRESS_DELAY_MS` (`delaysContentTouches`), so a
//!   card never lights up under a finger that is about to scroll. A control
//!   outside a list is pressed at once. A finger lifted before the delay
//!   still activates it.
//! - Contact controls, such as the Settings pads, act as the finger lands
//!   (`touchDown`) and take the rest of the contact.
//!
//! Focus is not touch. Buttons go to the focused control, the press events'
//! first responder; a touch moves focus only by activating a control, and
//! scrolling never does.
//!
//! A contact that outlives its screen ([`Owner`]: a page, a ride page, a
//! sheet opening or closing) is cancelled and gives nothing until the
//! finger lifts. A `Cancel` during a contact blocks it too, because input
//! may have been lost and a later point would otherwise look like a fresh
//! touch; with no contact it changes nothing. Producers end a cancelled
//! contact with a `Release` (see `Input::Cancel`). One that does not still
//! loses only that contact: a `Touch` at least `STALE_CONTACT_MS` after the
//! last touch edge starts a new one, while a finger that keeps reporting
//! stays blocked.
use super::home::Tile;
use device_api::input::{Input, Point, STALE_CONTACT_MS};
use heapless::Vec;

/// A tap moves less than this in either axis.
const TAP_SLOP: i32 = 16;
/// The finger travels this far along an axis before a list or a page claims
/// the contact (UIScrollView's pan hysteresis).
pub const DRAG_SLOP: i32 = super::kinetic::DRAG_SLOP;
/// Longer contacts are holds, not taps.
const TAP_MAX_MS: u64 = 600;
/// A contact this long, still within the tap slop, is a long press.
pub const HOLD_MS: u64 = 1_000;
/// A control in a list shows as pressed once the finger has rested this long.
pub const PRESS_DELAY_MS: u64 = 150;
/// A swipe travels at least this far sideways...
const SWIPE_MIN: i32 = 48;
/// ...at least twice as far as up or down, within this time.
const SWIPE_MAX_MS: u64 = 1000;

/// What the screen showed when an edge arrived; a contact that began under
/// one owner never completes under another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Owner(pub u32);

/// Something a tap acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Tile(Tile),
    /// Ready's workout line, which changes the workout.
    ReadyWorkout,
    /// Ready's sensors line, which pairs sensors.
    ReadySensors,
    /// The phone code, or anywhere once a session ended.
    Phone,
    /// A saved ride's card.
    Ride(usize),
    /// A workout's card.
    Workout(usize),
    /// A Settings row, by cursor position.
    Setting(usize),
    /// The number editor's minus and plus pads, which act on contact.
    Lower,
    Raise,
    /// A setup card.
    Setup(usize),
    /// The back-to-start page's pad.
    BackPad,
    /// The workout page's time left, which extends the step.
    ExtendStep,
    /// The map, which a long press stops following the course.
    Map,
    /// The sensor menu, which finds its own target where the finger landed.
    Menu(Point),
}

impl Control {
    /// Acts as the finger lands rather than when it lifts.
    pub const fn on_contact(self) -> bool {
        matches!(self, Self::Lower | Self::Raise)
    }
}

/// The lists that scroll under a finger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    Rides,
    Workouts,
    Settings,
}

/// What lies under a finger as it lands, deepest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hit {
    pub control: Option<Control>,
    /// The list the control sits in, or the list's empty space.
    pub list: Option<List>,
    /// The page takes sideways swipes.
    pub swipes: bool,
}

/// A swipe's direction, named by what it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swipe {
    /// Finger moved right to left: show what comes after.
    Next,
    /// Finger moved left to right: show what came before.
    Previous,
}

/// A finger's part in a list's scroll: the list follows it (`Kinetic`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pan {
    /// It landed at height `y`: moving content stops under it.
    Down(i32),
    Move(i32),
    /// It lifted.
    Up,
    /// The contact went elsewhere or was lost: no fling.
    Cancel,
}

/// What a contact tells the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchEvent {
    /// The control shows as pressed.
    Pressed(Control),
    /// It no longer does, and nothing happens.
    Released(Control),
    /// The finger lifted on it: act.
    Activated(Control),
    /// A contact control under a landing finger: act now.
    Contact(Control),
    /// The finger rested a second on it before lifting.
    Hold(Control),
    Pan(List, Pan),
    Swipe(Swipe),
}

/// The events of one edge: at most a release, an activation and a pan.
pub type Events = Vec<TouchEvent, 3>;

/// Who won the contact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    /// Still open: a tap, a hold, a pan or a swipe may yet win.
    Open,
    /// The list follows the finger.
    Pan,
    /// Sideways travel: a swipe if it lifts as one.
    Sideways,
    /// A contact control took it, or the finger left the slop with no one
    /// to claim it: nothing more comes of it.
    Spent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Contact {
    #[default]
    Idle,
    Down {
        owner: Owner,
        hit: Hit,
        start: Point,
        last: Point,
        since: u64,
        claim: Claim,
        /// The control shows as pressed.
        pressed: bool,
    },
    /// Ignore everything until the finger lifts.
    Blocked {
        /// The last touch edge of the blocked contact.
        last: u64,
        /// Cancelled input: a `Touch` after a stale gap is a new contact,
        /// for producers that cannot report the lift.
        cancelled: bool,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Touches {
    contact: Contact,
}

impl Touches {
    /// Feed one input edge seen under `owner`; `hit` names what lies under a
    /// landing finger. Buttons pass by.
    pub fn feed(
        &mut self,
        input: Input,
        now: u64,
        owner: Owner,
        hit: impl FnOnce(Point) -> Hit,
    ) -> Events {
        let mut events = Events::new();
        // A contact that outlived its screen is over for this screen.
        if let Contact::Down { owner: began, .. } = self.contact
            && began != owner
        {
            self.cancel_into(&mut events);
            self.contact = Contact::Blocked {
                last: now,
                cancelled: false,
            };
        }
        if let (
            Input::Touch(_),
            Contact::Blocked {
                last,
                cancelled: true,
            },
        ) = (input, self.contact)
            && now.saturating_sub(last) >= STALE_CONTACT_MS
        {
            self.contact = Contact::Idle;
        }
        match (input, self.contact) {
            (Input::Touch(point), Contact::Idle) => {
                self.land(point, now, owner, hit(point), &mut events)
            }
            (Input::Touch(point), Contact::Down { .. }) => self.slide(point, &mut events),
            (Input::Release, Contact::Down { .. }) => self.lift(now, &mut events),
            (Input::Release, _) => self.contact = Contact::Idle,
            // Lost input mid-contact: drop the rest of it.
            (Input::Cancel, Contact::Down { .. } | Contact::Blocked { .. }) => {
                self.cancel_into(&mut events);
                self.contact = Contact::Blocked {
                    last: now,
                    cancelled: true,
                };
            }
            // The blocked contact is still reporting.
            (Input::Touch(_), Contact::Blocked { cancelled, .. }) => {
                self.contact = Contact::Blocked {
                    last: now,
                    cancelled,
                };
            }
            _ => {}
        }
        events
    }

    /// The control under a resting finger shows as pressed once the scroll
    /// view's delay has passed. Call with the time before each frame.
    pub fn tick(&mut self, now: u64) -> Option<TouchEvent> {
        let Contact::Down {
            hit,
            since,
            claim: Claim::Open,
            pressed: pressed @ false,
            ..
        } = &mut self.contact
        else {
            return None;
        };
        let control = hit.control?;
        (now.saturating_sub(*since) >= PRESS_DELAY_MS).then(|| {
            *pressed = true;
            TouchEvent::Pressed(control)
        })
    }

    /// When `tick` will press a control, so a frame can be drawn then.
    pub fn due(&self) -> Option<u64> {
        match self.contact {
            Contact::Down {
                hit: Hit {
                    control: Some(_), ..
                },
                since,
                claim: Claim::Open,
                pressed: false,
                ..
            } => Some(since + PRESS_DELAY_MS),
            _ => None,
        }
    }

    /// The control shown as pressed.
    pub fn pressed(&self) -> Option<Control> {
        match self.contact {
            Contact::Down {
                hit, pressed: true, ..
            } => hit.control,
            _ => None,
        }
    }

    /// The list takes the contact now: a finger that stopped moving content
    /// is no tap (UIKit gives a touch on a decelerating scroll view to it).
    pub fn claim_pan(&mut self) {
        if let Contact::Down { claim, .. } = &mut self.contact
            && *claim == Claim::Open
        {
            *claim = Claim::Pan;
        }
    }

    fn land(&mut self, point: Point, now: u64, owner: Owner, hit: Hit, events: &mut Events) {
        let mut claim = Claim::Open;
        let mut pressed = false;
        if let Some(list) = hit.list {
            let _ = events.push(TouchEvent::Pan(list, Pan::Down(i32::from(point.y))));
        }
        match hit.control {
            Some(control) if control.on_contact() => {
                let _ = events.push(TouchEvent::Contact(control));
                claim = Claim::Spent;
            }
            // Outside a list nothing can scroll: press at once.
            Some(control) if hit.list.is_none() => {
                let _ = events.push(TouchEvent::Pressed(control));
                pressed = true;
            }
            _ => {}
        }
        self.contact = Contact::Down {
            owner,
            hit,
            start: point,
            last: point,
            since: now,
            claim,
            pressed,
        };
    }

    fn slide(&mut self, point: Point, events: &mut Events) {
        let Contact::Down {
            hit,
            start,
            last,
            claim,
            pressed,
            ..
        } = &mut self.contact
        else {
            return;
        };
        *last = point;
        let dx = i32::from(point.x) - i32::from(start.x);
        let dy = i32::from(point.y) - i32::from(start.y);
        if *claim == Claim::Open {
            let next = if hit.list.is_some() && dy.abs() >= DRAG_SLOP && dy.abs() >= dx.abs() {
                Claim::Pan
            } else if hit.swipes && dx.abs() >= DRAG_SLOP && dx.abs() > dy.abs() {
                Claim::Sideways
            } else if dx.abs() >= TAP_SLOP || dy.abs() >= TAP_SLOP {
                Claim::Spent
            } else {
                Claim::Open
            };
            if next != Claim::Open {
                *claim = next;
                if core::mem::take(pressed)
                    && let Some(control) = hit.control
                {
                    let _ = events.push(TouchEvent::Released(control));
                }
                // The list gives the finger up to the page's swipe.
                if next != Claim::Pan
                    && let Some(list) = hit.list
                {
                    let _ = events.push(TouchEvent::Pan(list, Pan::Cancel));
                }
            }
        }
        if matches!(*claim, Claim::Open | Claim::Pan)
            && let Some(list) = hit.list
        {
            let _ = events.push(TouchEvent::Pan(list, Pan::Move(i32::from(point.y))));
        }
    }

    fn lift(&mut self, now: u64, events: &mut Events) {
        let Contact::Down {
            hit,
            start,
            last,
            since,
            claim,
            pressed,
            ..
        } = self.contact
        else {
            return;
        };
        self.contact = Contact::Idle;
        let held = now.saturating_sub(since);
        if pressed && let Some(control) = hit.control {
            let _ = events.push(TouchEvent::Released(control));
        }
        match (claim, hit.control) {
            (Claim::Open, Some(control)) if held <= TAP_MAX_MS => {
                let _ = events.push(TouchEvent::Activated(control));
            }
            (Claim::Open, Some(control)) if held >= HOLD_MS => {
                let _ = events.push(TouchEvent::Hold(control));
            }
            (Claim::Sideways, _) => {
                let dx = i32::from(last.x) - i32::from(start.x);
                let dy = i32::from(last.y) - i32::from(start.y);
                if held <= SWIPE_MAX_MS && dx.abs() >= SWIPE_MIN && dx.abs() >= 2 * dy.abs() {
                    let swipe = if dx < 0 { Swipe::Next } else { Swipe::Previous };
                    let _ = events.push(TouchEvent::Swipe(swipe));
                }
            }
            _ => {}
        }
        if matches!(claim, Claim::Open | Claim::Pan)
            && let Some(list) = hit.list
        {
            let _ = events.push(TouchEvent::Pan(list, Pan::Up));
        }
    }

    /// The contact in progress is cancelled: its control is released and
    /// its list let go without a fling.
    fn cancel_into(&mut self, events: &mut Events) {
        if let Contact::Down {
            hit,
            claim,
            pressed,
            ..
        } = self.contact
        {
            if pressed && let Some(control) = hit.control {
                let _ = events.push(TouchEvent::Released(control));
            }
            if matches!(claim, Claim::Open | Claim::Pan)
                && let Some(list) = hit.list
            {
                let _ = events.push(TouchEvent::Pan(list, Pan::Cancel));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TouchEvent::{Activated, Hold, Pressed, Released};

    const A: Owner = Owner(1);
    const CARD: Control = Control::Ride(3);
    /// A card in a list on a page without swipes.
    const IN_LIST: Hit = Hit {
        control: Some(CARD),
        list: Some(List::Rides),
        swipes: false,
    };
    /// A control on a page that swipes, with no list.
    const ON_PAGE: Hit = Hit {
        control: Some(Control::BackPad),
        list: None,
        swipes: true,
    };

    fn at(x: u16, y: u16) -> Input {
        Input::Touch(Point { x, y })
    }

    /// Land on `hit` at the first point, move through the rest over `ms`,
    /// then lift; every event in order, with the press delay ticked between.
    fn contact(
        touches: &mut Touches,
        hit: Hit,
        points: &[(u16, u16)],
        ms: u64,
    ) -> std::vec::Vec<TouchEvent> {
        let step = ms / points.len() as u64;
        let mut events = std::vec::Vec::new();
        for (i, &(x, y)) in points.iter().enumerate() {
            let now = i as u64 * step;
            events.extend(touches.tick(now));
            events.extend(touches.feed(at(x, y), now, A, |_| hit));
        }
        events.extend(touches.tick(ms));
        events.extend(touches.feed(Input::Release, ms, A, |_| hit));
        events
    }

    /// The events other than the list's pan.
    fn taps(events: std::vec::Vec<TouchEvent>) -> std::vec::Vec<TouchEvent> {
        events
            .into_iter()
            .filter(|e| !matches!(e, TouchEvent::Pan(..)))
            .collect()
    }

    #[test]
    fn a_quick_tap_activates_the_control_it_landed_on() {
        let mut t = Touches::default();
        let events = contact(&mut t, IN_LIST, &[(100, 200), (104, 203)], 120);
        assert_eq!(taps(events), [Activated(CARD)], "lifted before the delay");
        // Outside a list the control is pressed at once.
        let events = contact(&mut t, ON_PAGE, &[(100, 200)], 80);
        assert_eq!(
            events,
            [
                Pressed(Control::BackPad),
                Released(Control::BackPad),
                Activated(Control::BackPad)
            ]
        );
        // A hold between a tap and a long press is nothing.
        assert_eq!(
            taps(contact(&mut t, IN_LIST, &[(100, 200)], 900)),
            [Pressed(CARD), Released(CARD)]
        );
    }

    #[test]
    fn a_card_in_a_list_lights_up_only_once_the_finger_rests() {
        let mut t = Touches::default();
        t.feed(at(100, 200), 0, A, |_| IN_LIST);
        assert_eq!(t.due(), Some(PRESS_DELAY_MS));
        assert_eq!(t.tick(PRESS_DELAY_MS - 1), None);
        assert_eq!(t.pressed(), None);
        assert_eq!(t.tick(PRESS_DELAY_MS), Some(Pressed(CARD)));
        assert_eq!(t.pressed(), Some(CARD));
        assert_eq!(t.due(), None);
        let up = t.feed(Input::Release, 300, A, |_| IN_LIST);
        assert_eq!(taps(up.to_vec()), [Released(CARD), Activated(CARD)]);
    }

    #[test]
    fn scrolling_takes_the_contact_from_the_card_and_never_activates_it() {
        let mut t = Touches::default();
        // Pressed, then the finger moves off up the list.
        let events = contact(
            &mut t,
            IN_LIST,
            &[(100, 200), (100, 200), (101, 180), (101, 120)],
            600,
        );
        assert_eq!(taps(events.clone()), [Pressed(CARD), Released(CARD)]);
        let pans: std::vec::Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                TouchEvent::Pan(List::Rides, pan) => Some(*pan),
                _ => None,
            })
            .collect();
        assert_eq!(pans.first(), Some(&Pan::Down(200)));
        assert_eq!(pans.last(), Some(&Pan::Up));
        assert!(pans.contains(&Pan::Move(120)));
        // A quick flick before the delay: no press, no activation.
        let flick = contact(&mut t, IN_LIST, &[(100, 250), (100, 200), (100, 120)], 60);
        assert_eq!(taps(flick), []);
        // A finger that stopped moving content is the list's at once.
        t.feed(at(100, 200), 0, A, |_| IN_LIST);
        t.claim_pan();
        assert_eq!(t.tick(500), None, "never pressed");
        let up = t.feed(Input::Release, 520, A, |_| IN_LIST);
        assert_eq!(up.to_vec(), [TouchEvent::Pan(List::Rides, Pan::Up)]);
    }

    #[test]
    fn sideways_travel_is_the_pages_swipe_and_cancels_the_list() {
        let mut t = Touches::default();
        let left = contact(&mut t, ON_PAGE, &[(200, 150), (150, 155), (90, 160)], 300);
        assert_eq!(
            left,
            [
                Pressed(Control::BackPad),
                Released(Control::BackPad),
                TouchEvent::Swipe(Swipe::Next)
            ]
        );
        let right = contact(&mut t, ON_PAGE, &[(40, 150), (120, 140)], 300);
        assert_eq!(right.last(), Some(&TouchEvent::Swipe(Swipe::Previous)));
        // Over a list on a page that swipes, the list lets go.
        let both = Hit {
            swipes: true,
            ..IN_LIST
        };
        let events = contact(&mut t, both, &[(200, 150), (150, 152), (60, 155)], 200);
        assert!(events.contains(&TouchEvent::Pan(List::Rides, Pan::Cancel)));
        assert_eq!(events.last(), Some(&TouchEvent::Swipe(Swipe::Next)));
        assert!(!events.contains(&TouchEvent::Pan(List::Rides, Pan::Up)));
        // Sideways on a page without swipes, past the slop: nothing.
        let none = contact(&mut t, IN_LIST, &[(200, 150), (150, 152), (60, 155)], 200);
        assert_eq!(taps(none), []);
    }

    #[test]
    fn short_slow_or_diagonal_drags_are_nothing() {
        let mut t = Touches::default();
        for (points, ms) in [
            (&[(100, 100), (70, 100)][..], 200),
            (&[(200, 100), (100, 100)][..], 1500),
            (&[(200, 100), (140, 140)][..], 200),
            // Out and back is a drag that returned, not a tap.
            (&[(40, 80), (180, 80), (40, 80)][..], 150),
        ] {
            let events = contact(&mut t, ON_PAGE, points, ms);
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, Activated(_) | TouchEvent::Swipe(_))),
                "{points:?} {events:?}"
            );
        }
    }

    #[test]
    fn a_still_second_is_a_long_press_and_a_wandering_one_is_nothing() {
        let mut t = Touches::default();
        let map = Hit {
            control: Some(Control::Map),
            list: None,
            swipes: true,
        };
        let events = contact(&mut t, map, &[(100, 200), (103, 202), (101, 199)], 1_200);
        assert_eq!(events.last(), Some(&Hold(Control::Map)));
        let wandering = contact(&mut t, map, &[(100, 200), (130, 200), (100, 200)], 1_200);
        assert!(!wandering.contains(&Hold(Control::Map)));
    }

    #[test]
    fn a_contact_control_acts_as_the_finger_lands_and_takes_the_rest() {
        let mut t = Touches::default();
        let pad = Hit {
            control: Some(Control::Raise),
            list: None,
            swipes: false,
        };
        let events = contact(&mut t, pad, &[(200, 220), (201, 221)], 100);
        assert_eq!(events, [TouchEvent::Contact(Control::Raise)]);
    }

    #[test]
    fn cancel_blocks_until_the_finger_lifts() {
        let mut t = Touches::default();
        t.feed(at(200, 150), 0, A, |_| IN_LIST);
        t.tick(200);
        let cancelled = t.feed(Input::Cancel, 250, A, |_| IN_LIST);
        assert_eq!(
            cancelled.to_vec(),
            [Released(CARD), TouchEvent::Pan(List::Rides, Pan::Cancel)]
        );
        // The rest of a swipe that lost its start must not read as a tap.
        assert!(t.feed(at(80, 150), 300, A, |_| ON_PAGE).is_empty());
        assert!(t.feed(Input::Release, 350, A, |_| ON_PAGE).is_empty());
        // The next contact is whole again.
        let tap = contact(&mut t, ON_PAGE, &[(50, 50)], 100);
        assert_eq!(tap.last(), Some(&Activated(Control::BackPad)));
        // A stray release or a button changes nothing.
        let button = Input::Button {
            button: device_api::input::Button::BottomLeft,
            code: 1,
        };
        assert!(t.feed(Input::Release, 200, A, |_| ON_PAGE).is_empty());
        assert!(t.feed(button, 210, A, |_| ON_PAGE).is_empty());
    }

    /// Each row: edges in order, each with the owner on screen then and its
    /// time, and whether the last edge activates the control.
    #[test]
    fn contacts_belong_to_the_screen_they_began_on() {
        const B: Owner = Owner(2);
        let stale = STALE_CONTACT_MS;
        type Row<'a> = (&'a str, &'a [(Input, Owner, u64)], bool);
        let table: &[Row] = &[
            (
                "begin on A, a button opens page or subview B, release",
                &[(at(50, 50), A, 0), (Input::Release, B, 100)],
                false,
            ),
            (
                "begin on A, move after navigating, release",
                &[
                    (at(50, 50), A, 0),
                    (at(52, 50), B, 50),
                    (Input::Release, B, 100),
                ],
                false,
            ),
            (
                "navigate, the finger rests past the stale time, moves, lifts",
                &[
                    (at(50, 50), A, 0),
                    (at(51, 50), B, 50),
                    (at(52, 50), B, 50 + 2 * stale),
                    (Input::Release, B, 60 + 2 * stale),
                ],
                false,
            ),
            (
                "begin, a sheet opens, release over it, dismiss, tap",
                &[
                    (at(50, 50), A, 0),
                    (Input::Release, B, 100),
                    (at(50, 50), A, 200),
                    (Input::Release, A, 300),
                ],
                true,
            ),
            (
                "begin on the sheet, it closes, release",
                &[(at(50, 50), B, 0), (Input::Release, A, 100)],
                false,
            ),
            (
                "idle Cancel, then a tap",
                &[
                    (Input::Cancel, A, 0),
                    (at(50, 50), A, 10),
                    (Input::Release, A, 100),
                ],
                true,
            ),
            (
                "Touch, Cancel, Touch without a gap, Release",
                &[
                    (at(50, 50), A, 0),
                    (Input::Cancel, A, 10),
                    (at(50, 50), A, 20),
                    (Input::Release, A, 100),
                ],
                false,
            ),
            (
                "Touch, Cancel, Release, then a tap",
                &[
                    (at(200, 50), A, 0),
                    (Input::Cancel, A, 10),
                    (Input::Release, A, 20),
                    (at(50, 50), A, 30),
                    (Input::Release, A, 100),
                ],
                true,
            ),
            (
                "Touch, Cancel, Touch after a stale gap, Release",
                &[
                    (at(200, 50), A, 0),
                    (Input::Cancel, A, 10),
                    (at(50, 50), A, 10 + stale),
                    (Input::Release, A, 100 + stale),
                ],
                true,
            ),
            (
                "Touch, Cancel, the release suppressed, a new Touch, Release",
                &[
                    (at(200, 50), A, 0),
                    (at(120, 50), A, 50),
                    (Input::Cancel, A, 60),
                    (at(50, 50), A, 60 + 3 * stale),
                    (Input::Release, A, 160 + 3 * stale),
                ],
                true,
            ),
            (
                "Cancel on a stale contact, then a tap",
                &[
                    (at(200, 50), A, 0),
                    (Input::Cancel, B, 10),
                    (Input::Release, B, 20),
                    (at(50, 50), B, 30),
                    (Input::Release, B, 100),
                ],
                true,
            ),
        ];
        for (what, edges, want) in table {
            let mut t = Touches::default();
            let mut last = Events::new();
            for &(input, owner, now) in edges.iter() {
                last = t.feed(input, now, owner, |_| ON_PAGE);
            }
            assert_eq!(last.contains(&Activated(Control::BackPad)), *want, "{what}");
        }
    }

    /// A cancelled finger that keeps sliding never fires on lift, however
    /// long it slides, as long as it keeps reporting.
    #[test]
    fn a_cancelled_finger_that_keeps_sliding_never_fires() {
        let mut t = Touches::default();
        t.feed(at(100, 100), 0, A, |_| ON_PAGE);
        t.feed(Input::Cancel, 10, A, |_| ON_PAGE);
        for step in 1..=200u16 {
            let x = 100 + step % 3;
            assert!(
                t.feed(at(x, 100), 10 + 10 * u64::from(step), A, |_| ON_PAGE)
                    .is_empty()
            );
        }
        assert!(t.feed(Input::Release, 2020, A, |_| ON_PAGE).is_empty());
        let tap = contact(&mut t, ON_PAGE, &[(60, 60)], 100);
        assert_eq!(tap.last(), Some(&Activated(Control::BackPad)));
    }

    /// The driver's contact, the bounded queue and the arbiter together, as
    /// on the device: a slow consumer whose queue overflows mid-contact never
    /// sees a retained move as a fresh touch.
    #[test]
    fn an_overflowing_queue_never_turns_a_cancelled_contact_into_a_tap() {
        use firmware_services::input::{Edges, Poll, TouchContact};
        fn produce(contact: &mut TouchContact, edges: &mut Edges, poll: Poll, now: u64) {
            for input in contact.poll(poll, now).into_iter().flatten() {
                if edges.push(now, input) {
                    contact.cancel(now);
                }
            }
        }
        fn consume(touches: &mut Touches, edges: &mut Edges) -> std::vec::Vec<TouchEvent> {
            core::iter::from_fn(|| edges.pop())
                .flat_map(|edge| touches.feed(edge.input, edge.received_ms, A, |_| ON_PAGE))
                .filter(|event| matches!(event, Activated(_)))
                .collect()
        }
        let point = |x| Poll::Press(Point { x, y: 100 });
        for consumed_down in [false, true] {
            let (mut contact, mut edges, mut touches) =
                (TouchContact::default(), Edges::new(), Touches::default());
            produce(&mut contact, &mut edges, point(100), 0);
            if consumed_down {
                assert_eq!(consume(&mut touches, &mut edges), []);
            }
            // The consumer stalls while the finger wobbles in place, and the
            // queue overflows; then the finger lifts.
            for step in 1..40u16 {
                produce(
                    &mut contact,
                    &mut edges,
                    point(100 + step % 2),
                    u64::from(step) * 10,
                );
            }
            produce(&mut contact, &mut edges, Poll::Release, 400);
            assert_eq!(
                consume(&mut touches, &mut edges),
                [],
                "down consumed: {consumed_down}"
            );
            // The next tap is whole.
            produce(&mut contact, &mut edges, point(60), 500);
            produce(&mut contact, &mut edges, Poll::Release, 550);
            assert_eq!(
                consume(&mut touches, &mut edges),
                [Activated(Control::BackPad)]
            );
        }
    }
}
