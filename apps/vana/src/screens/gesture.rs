//! Touch gestures from raw contact edges: a tap, a long press or a
//! horizontal swipe.
//!
//! Policy lives with the caller: taps select, swipes change pages, and no
//! gesture ever controls a ride.
//!
//! Every touch edge reaches the recognizer, whatever owns input, together
//! with the [`Owner`] of the screen shown at that moment. A contact belongs to
//! the owner it began on: once the owner changes (a page, a ride page, a
//! sheet opening or closing) the contact is cancelled and gives nothing until
//! the finger lifts.
//!
//! A `Cancel` during a contact blocks it too, because input may have been
//! lost and a later point would otherwise look like a fresh touch; with no
//! contact it changes nothing. Producers end a cancelled contact with a
//! `Release` (see `Input::Cancel`). One that does not still loses only that
//! contact: a `Touch` at least `STALE_CONTACT_MS` after the last touch edge
//! starts a new one, while a finger that keeps reporting stays blocked.
use device_api::input::{Input, Point, STALE_CONTACT_MS};

/// A tap moves less than this in either axis.
const TAP_SLOP: i32 = 16;
/// Longer contacts are holds, not taps.
const TAP_MAX_MS: u64 = 600;
/// A contact this long, still within the tap slop, is a long press.
pub const HOLD_MS: u64 = 1_000;
/// A swipe travels at least this far horizontally...
const SWIPE_MIN: i32 = 48;
/// ...at least twice as far as vertically, within this time.
const SWIPE_MAX_MS: u64 = 1000;

/// What the screen showed when an edge arrived; a contact that began under
/// one owner never completes under another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Owner(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// A contact began here: controls that act on contact use this.
    Down(Point),
    Tap(Point),
    /// A finger held still for `HOLD_MS` or more, ending as it lifts: deliberate,
    /// so a raindrop or a bump does not make one.
    Hold(Point),
    /// Finger moved right to left: show what comes after.
    Next,
    /// Finger moved left to right: show what came before.
    Previous,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Contact {
    #[default]
    Idle,
    Down {
        owner: Owner,
        start: Point,
        last: Point,
        since: u64,
        /// Some point so far left the tap slop, so this is no longer a tap.
        wandered: bool,
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
pub struct Gestures {
    contact: Contact,
}

impl Gestures {
    /// Feed one input edge seen under `owner`; a gesture completes on
    /// release. Buttons pass by.
    pub fn feed(&mut self, input: Input, now: u64, owner: Owner) -> Option<Gesture> {
        // A contact that outlived its screen is over for this screen.
        if let Contact::Down { owner: began, .. } = self.contact
            && began != owner
        {
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
                self.contact = Contact::Down {
                    owner,
                    start: point,
                    last: point,
                    since: now,
                    wandered: false,
                };
                Some(Gesture::Down(point))
            }
            (
                Input::Touch(point),
                Contact::Down {
                    start,
                    since,
                    wandered,
                    ..
                },
            ) => {
                self.contact = Contact::Down {
                    owner,
                    start,
                    last: point,
                    since,
                    wandered: wandered || !within_slop(start, point),
                };
                None
            }
            (
                Input::Release,
                Contact::Down {
                    start,
                    last,
                    since,
                    wandered,
                    ..
                },
            ) => {
                self.contact = Contact::Idle;
                classify(start, last, now.saturating_sub(since), wandered)
            }
            (Input::Release, _) => {
                self.contact = Contact::Idle;
                None
            }
            // Lost input mid-contact: drop the rest of it.
            (Input::Cancel, Contact::Down { .. } | Contact::Blocked { .. }) => {
                self.contact = Contact::Blocked {
                    last: now,
                    cancelled: true,
                };
                None
            }
            // The blocked contact is still reporting.
            (Input::Touch(_), Contact::Blocked { cancelled, .. }) => {
                self.contact = Contact::Blocked {
                    last: now,
                    cancelled,
                };
                None
            }
            _ => None,
        }
    }
}

fn within_slop(start: Point, point: Point) -> bool {
    let dx = i32::from(point.x) - i32::from(start.x);
    let dy = i32::from(point.y) - i32::from(start.y);
    dx.abs() < TAP_SLOP && dy.abs() < TAP_SLOP
}

/// A tap never left the slop on the way; a swipe is judged from start to end.
fn classify(start: Point, end: Point, held_ms: u64, wandered: bool) -> Option<Gesture> {
    let dx = i32::from(end.x) - i32::from(start.x);
    let dy = i32::from(end.y) - i32::from(start.y);
    if within_slop(start, end) {
        return match (wandered, held_ms) {
            (true, _) => None,
            (false, ..=TAP_MAX_MS) => Some(Gesture::Tap(start)),
            (false, HOLD_MS..) => Some(Gesture::Hold(start)),
            _ => None,
        };
    }
    if held_ms <= SWIPE_MAX_MS && dx.abs() >= SWIPE_MIN && dx.abs() >= 2 * dy.abs() {
        return Some(if dx < 0 {
            Gesture::Next
        } else {
            Gesture::Previous
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Owner = Owner(1);

    fn at(x: u16, y: u16) -> Input {
        Input::Touch(Point { x, y })
    }

    /// Touch down at the first point, move through the rest, then lift.
    fn contact(gestures: &mut Gestures, points: &[(u16, u16)], ms: u64) -> Option<Gesture> {
        let step = ms / points.len() as u64;
        for (i, &(x, y)) in points.iter().enumerate() {
            let edge = gestures.feed(at(x, y), i as u64 * step, A);
            let down = (i == 0).then_some(Gesture::Down(Point { x, y }));
            assert_eq!(edge, down);
        }
        gestures.feed(Input::Release, ms, A)
    }

    #[test]
    fn short_still_contact_is_a_tap_where_it_started() {
        let mut g = Gestures::default();
        assert_eq!(
            contact(&mut g, &[(100, 200), (104, 203)], 120),
            Some(Gesture::Tap(Point { x: 100, y: 200 }))
        );
        // A hold between a tap and a long press is nothing.
        assert_eq!(contact(&mut g, &[(100, 200)], 900), None);
    }

    #[test]
    fn a_still_second_is_a_long_press_and_a_wandering_one_is_nothing() {
        let mut g = Gestures::default();
        assert_eq!(
            contact(&mut g, &[(100, 200), (103, 202), (101, 199)], 1_200),
            Some(Gesture::Hold(Point { x: 100, y: 200 }))
        );
        let wandering = [(100, 200), (130, 200), (100, 200)];
        assert_eq!(contact(&mut g, &wandering, 1_200), None);
    }

    #[test]
    fn horizontal_travel_is_a_swipe_and_its_direction_names_the_page() {
        let mut g = Gestures::default();
        let left = [(200, 150), (150, 155), (90, 160)];
        assert_eq!(contact(&mut g, &left, 300), Some(Gesture::Next));
        let right = [(40, 150), (120, 140)];
        assert_eq!(contact(&mut g, &right, 300), Some(Gesture::Previous));
    }

    #[test]
    fn short_slow_or_diagonal_drags_are_nothing() {
        let mut g = Gestures::default();
        assert_eq!(contact(&mut g, &[(100, 100), (70, 100)], 200), None);
        assert_eq!(contact(&mut g, &[(200, 100), (100, 100)], 1500), None);
        assert_eq!(contact(&mut g, &[(200, 100), (140, 140)], 200), None);
        assert_eq!(contact(&mut g, &[(100, 40), (100, 200)], 200), None);
        // Out and back is a drag that returned, not a tap.
        assert_eq!(contact(&mut g, &[(40, 80), (180, 80), (40, 80)], 150), None);
    }

    #[test]
    fn cancel_blocks_until_the_finger_lifts() {
        let mut g = Gestures::default();
        g.feed(at(200, 150), 0, A);
        assert_eq!(g.feed(Input::Cancel, 50, A), None);
        // The rest of a swipe that lost its start must not read as a tap.
        g.feed(at(80, 150), 100, A);
        assert_eq!(g.feed(Input::Release, 150, A), None);
        // The next contact is whole again.
        assert_eq!(
            contact(&mut g, &[(50, 50)], 100),
            Some(Gesture::Tap(Point { x: 50, y: 50 }))
        );
        // A stray release or a button changes nothing.
        let button = Input::Button {
            button: device_api::input::Button::BottomLeft,
            code: 1,
        };
        assert_eq!(g.feed(Input::Release, 200, A), None);
        assert_eq!(g.feed(button, 210, A), None);
    }

    /// Each row: edges in order, each with the owner on screen then and its
    /// time, and the gesture the last edge must give.
    #[test]
    fn contacts_belong_to_the_screen_they_began_on() {
        const B: Owner = Owner(2);
        let tap = Some(Gesture::Tap(Point { x: 50, y: 50 }));
        let stale = STALE_CONTACT_MS;
        type Row<'a> = (&'a str, &'a [(Input, Owner, u64)], Option<Gesture>);
        let table: &[Row] = &[
            (
                "begin on A, a button opens page or subview B, release",
                &[(at(50, 50), A, 0), (Input::Release, B, 100)],
                None,
            ),
            (
                "begin on A, move after navigating, release",
                &[
                    (at(50, 50), A, 0),
                    (at(52, 50), B, 50),
                    (Input::Release, B, 100),
                ],
                None,
            ),
            (
                "navigate, the finger rests past the stale time, moves, lifts",
                &[
                    (at(50, 50), A, 0),
                    (at(51, 50), B, 50),
                    (at(52, 50), B, 50 + 2 * stale),
                    (Input::Release, B, 60 + 2 * stale),
                ],
                None,
            ),
            (
                "begin, a sheet opens, release over it, dismiss, tap",
                &[
                    (at(50, 50), A, 0),
                    (Input::Release, B, 100),
                    (at(50, 50), A, 200),
                    (Input::Release, A, 300),
                ],
                tap,
            ),
            (
                "begin on the sheet, it closes, release",
                &[(at(50, 50), B, 0), (Input::Release, A, 100)],
                None,
            ),
            (
                "idle Cancel, then a tap",
                &[
                    (Input::Cancel, A, 0),
                    (at(50, 50), A, 10),
                    (Input::Release, A, 100),
                ],
                tap,
            ),
            (
                "Touch, Cancel, Touch without a gap, Release",
                &[
                    (at(50, 50), A, 0),
                    (Input::Cancel, A, 10),
                    (at(50, 50), A, 20),
                    (Input::Release, A, 100),
                ],
                None,
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
                tap,
            ),
            (
                "Touch, Cancel, Touch after a stale gap, Release",
                &[
                    (at(200, 50), A, 0),
                    (Input::Cancel, A, 10),
                    (at(50, 50), A, 10 + stale),
                    (Input::Release, A, 100 + stale),
                ],
                tap,
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
                tap,
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
                tap,
            ),
        ];
        for (what, edges, want) in table {
            let mut g = Gestures::default();
            let mut last = None;
            for &(input, owner, now) in edges.iter() {
                last = g.feed(input, now, owner);
            }
            assert_eq!(last, *want, "{what}");
        }
    }

    /// A cancelled finger that keeps sliding never fires on lift, however
    /// long it slides, as long as it keeps reporting.
    #[test]
    fn a_cancelled_finger_that_keeps_sliding_never_fires() {
        let mut g = Gestures::default();
        g.feed(at(100, 100), 0, A);
        g.feed(Input::Cancel, 10, A);
        for step in 1..=200u16 {
            let x = 100 + step % 3;
            assert_eq!(g.feed(at(x, 100), 10 + 10 * u64::from(step), A), None);
        }
        assert_eq!(g.feed(Input::Release, 2020, A), None);
        assert_eq!(
            contact(&mut g, &[(60, 60)], 100),
            Some(Gesture::Tap(Point { x: 60, y: 60 }))
        );
    }

    /// The driver's contact, the bounded queue and the recognizer together,
    /// as on the device: a slow consumer whose queue overflows mid-contact
    /// never sees a retained move as a fresh touch.
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
        fn consume(gestures: &mut Gestures, edges: &mut Edges) -> std::vec::Vec<Gesture> {
            core::iter::from_fn(|| edges.pop())
                .filter_map(|edge| gestures.feed(edge.input, edge.received_ms, A))
                .filter(|gesture| !matches!(gesture, Gesture::Down(_)))
                .collect()
        }
        let point = |x| Poll::Press(Point { x, y: 100 });
        for consumed_down in [false, true] {
            let (mut contact, mut edges, mut gestures) =
                (TouchContact::default(), Edges::new(), Gestures::default());
            produce(&mut contact, &mut edges, point(100), 0);
            if consumed_down {
                assert_eq!(consume(&mut gestures, &mut edges), []);
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
                consume(&mut gestures, &mut edges),
                [],
                "down consumed: {consumed_down}"
            );
            // The next tap is whole.
            produce(&mut contact, &mut edges, point(60), 500);
            produce(&mut contact, &mut edges, Poll::Release, 550);
            assert_eq!(
                consume(&mut gestures, &mut edges),
                [Gesture::Tap(Point { x: 60, y: 100 })]
            );
        }
    }
}
