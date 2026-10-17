//! Bounded physical input delivery with explicit cancellation on overflow,
//! and the touch contact that keeps `Input::Cancel`'s contract.
use device_api::input::Edge;
use device_api::input::Input;
use device_api::input::{Point, STALE_CONTACT_MS};
use heapless::Deque;
/// Single-consumer bounded edge queue. On overflow discard queued history and
/// enqueue Cancel before the newest event, so consumers cannot retain old holds.
///
/// An overflow during a touch contact cancels that contact for good, as
/// `Input::Cancel` requires: its later points are dropped until its
/// `Release`, which passes, or until a point arrives `STALE_CONTACT_MS` after
/// the last one, which starts a new contact. A consumer can therefore never
/// mistake a retained move for a fresh touch. `push` reports the overflow so
/// a producer can cancel its own contact too (`TouchContact::cancel`).
/// Acquisition never waits for a consumer. `lost` counts discarded input edges.
pub struct Edges {
    values: Deque<Edge, 16>,
    sequence: u32,
    pub lost: u32,
    /// A touch contact is in progress: its last edge was a point.
    touching: bool,
    /// An overflow cancelled the contact in progress; drop its points.
    suppressing: bool,
    last_touch_ms: u64,
}
impl Default for Edges {
    fn default() -> Self {
        Self::new()
    }
}
impl Edges {
    pub const fn new() -> Self {
        Self {
            values: Deque::new(),
            sequence: 0,
            lost: 0,
            touching: false,
            suppressing: false,
            last_touch_ms: 0,
        }
    }
    /// Queue `input`; returns whether the queue overflowed and cancelled
    /// held input, so the producer can cancel its touch contact too.
    #[must_use]
    pub fn push(&mut self, received_ms: u64, input: Input) -> bool {
        if self.suppressing && matches!(input, Input::Touch(_)) {
            if received_ms.saturating_sub(self.last_touch_ms) < STALE_CONTACT_MS {
                self.last_touch_ms = received_ms;
                self.lost = self.lost.saturating_add(1);
                return false;
            }
            // Points after a stale gap are a new contact.
            self.suppressing = false;
        }
        let overflowed = self.values.is_full();
        if overflowed {
            self.lost = self.lost.saturating_add(self.values.len() as u32);
            self.values.clear();
            self.insert(received_ms, Input::Cancel);
            // The contact in progress lost its history: drop the rest of it.
            self.suppressing = self.touching || matches!(input, Input::Touch(_));
        }
        match input {
            Input::Touch(_) => {
                self.touching = true;
                self.last_touch_ms = received_ms;
                if self.suppressing {
                    self.lost = self.lost.saturating_add(1);
                    return overflowed;
                }
            }
            Input::Release => {
                self.touching = false;
                self.suppressing = false;
            }
            Input::Cancel | Input::Button { .. } => {}
        }
        self.insert(received_ms, input);
        overflowed
    }
    fn insert(&mut self, received_ms: u64, input: Input) {
        self.sequence = self.sequence.wrapping_add(1);
        // Cannot fail: push clears a full queue first.
        let _ = self.values.push_back(Edge {
            sequence: self.sequence,
            received_ms,
            input,
        });
    }
    pub fn pop(&mut self) -> Option<Edge> {
        self.values.pop_front()
    }
}

/// One poll of a touch panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    Press(Point),
    Release,
    /// A report that says nothing, such as a malformed one.
    Nothing,
    /// The panel could not be read.
    Failed,
}

/// Turns polled touch reports into `Input` edges, keeping the
/// `Input::Cancel` contract: once a contact is cancelled its later presses
/// send nothing, and one `Release` ends it when the panel reports the lift or
/// has not reported the finger for `STALE_CONTACT_MS`. The next press then
/// starts a fresh contact.
#[derive(Clone, Copy, Debug, Default)]
pub struct TouchContact {
    point: Option<Point>,
    last_press: u64,
    /// When the contact in progress was cancelled.
    cancelled: Option<u64>,
}

impl TouchContact {
    /// One poll at `now`; the edges to send, in order.
    pub fn poll(&mut self, poll: Poll, now: u64) -> [Option<Input>; 3] {
        let edge = match poll {
            Poll::Press(next) => {
                self.last_press = now;
                if self.cancelled.is_some() || self.point == Some(next) {
                    None
                } else {
                    self.point = Some(next);
                    Some(Input::Touch(next))
                }
            }
            Poll::Release => {
                let ended = self.cancelled.take().is_some() | self.point.take().is_some();
                ended.then_some(Input::Release)
            }
            Poll::Nothing => None,
            Poll::Failed => self.cancel(now).then_some(Input::Cancel),
        };
        let [cancel, end] = self.tick(now);
        [edge, cancel, end]
    }

    /// Input was lost (a poll failure, companion framing, a power boundary or
    /// a queue overflow): cancel the contact in progress. Returns whether
    /// there was one; the caller makes sure a `Cancel` is sent.
    pub fn cancel(&mut self, now: u64) -> bool {
        let touching = self.point.take().is_some();
        if touching {
            self.cancelled = Some(now);
        }
        touching
    }

    /// A finger the panel stopped reporting is gone: its contact is
    /// cancelled and ends at once, so the next press is a whole new contact.
    /// A cancelled one ends once its presses have stopped as long.
    fn tick(&mut self, now: u64) -> [Option<Input>; 2] {
        let quiet = now.saturating_sub(self.last_press) > STALE_CONTACT_MS;
        if self.point.is_some() && quiet {
            self.point = None;
            self.cancelled = None;
            return [Some(Input::Cancel), Some(Input::Release)];
        }
        if self.cancelled.is_some() && quiet {
            self.cancelled = None;
            return [Some(Input::Release), None];
        }
        [None, None]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_api::input::Point;
    #[test]
    fn slow_consumer_gets_cancel_before_new_input() {
        let mut edges = Edges::new();
        for t in 0..16 {
            assert!(!edges.push(t, Input::Touch(Point { x: t as u16, y: 0 })));
        }
        assert!(edges.push(16, Input::Release));
        let cancel = edges.pop().unwrap();
        assert_eq!(cancel.input, Input::Cancel);
        assert_eq!(cancel.sequence, 17);
        assert_eq!(edges.pop().unwrap().input, Input::Release);
        assert!(edges.pop().is_none());
        assert_eq!(edges.lost, 16);
    }
    #[test]
    fn repeated_wraps_preserve_fifo() {
        let mut edges = Edges::new();
        for t in 0..100 {
            assert!(!edges.push(t, Input::Release));
            assert_eq!(edges.pop().unwrap().received_ms, t);
        }
        assert_eq!(edges.lost, 0);
    }

    #[test]
    fn an_overflow_cancels_the_contact_through_its_release() {
        let point = |x| Input::Touch(Point { x, y: 1 });
        let drain = |edges: &mut Edges| {
            core::iter::from_fn(|| edges.pop().map(|edge| edge.input)).collect::<std::vec::Vec<_>>()
        };
        // The contact's down and its first moves are lost to the overflow;
        // its later moves never reach the consumer, its lift does.
        let mut edges = Edges::new();
        for t in 0..16 {
            assert!(!edges.push(t, point(t as u16)));
        }
        assert!(edges.push(16, point(16)));
        assert!(!edges.push(26, point(17)));
        assert!(!edges.push(36, Input::Release));
        assert_eq!(drain(&mut edges), [Input::Cancel, Input::Release]);
        assert_eq!(edges.lost, 18);
        // The next contact is whole.
        assert!(!edges.push(50, point(5)));
        assert!(!edges.push(60, Input::Release));
        assert_eq!(drain(&mut edges), [point(5), Input::Release]);

        // An overflow of buttons during a contact cancels it too.
        let mut edges = Edges::new();
        assert!(!edges.push(0, point(1)));
        let _ = edges.pop();
        let button = Input::Button {
            button: device_api::input::Button::BottomLeft,
            code: 1,
        };
        for t in 1..=16 {
            assert!(!edges.push(t, button));
        }
        assert!(edges.push(17, button));
        assert!(!edges.push(20, point(2)));
        assert_eq!(drain(&mut edges), [Input::Cancel, button]);

        // A producer that cannot report the lift: a point after a stale gap
        // starts a new contact.
        assert!(!edges.push(20 + STALE_CONTACT_MS, point(3)));
        assert_eq!(drain(&mut edges), [point(3)]);
    }

    const fn at(x: u16) -> Point {
        Point { x, y: 100 }
    }

    /// Feed (ms, poll) pairs; collect the edges.
    fn edges(contact: &mut TouchContact, polls: &[(u64, Poll)]) -> std::vec::Vec<Input> {
        polls
            .iter()
            .flat_map(|&(now, poll)| contact.poll(poll, now))
            .flatten()
            .collect()
    }

    #[test]
    fn a_cancelled_contact_sends_nothing_more_and_ends_with_one_release() {
        use Poll::{Failed, Nothing, Press, Release};
        // A poll failure mid-contact: the finger keeps sliding, then lifts.
        let mut contact = TouchContact::default();
        let polls = [
            (0, Press(at(10))),
            (10, Failed),
            (20, Press(at(30))),
            (30, Press(at(60))),
            (40, Release),
            (50, Press(at(90))),
        ];
        assert_eq!(
            edges(&mut contact, &polls),
            [
                Input::Touch(at(10)),
                Input::Cancel,
                Input::Release,
                Input::Touch(at(90))
            ]
        );

        // A failure with the finger still down and the panel unreadable: the
        // contact ends once its presses have been missing for the stale time.
        let mut contact = TouchContact::default();
        let mut polls = std::vec![(0, Press(at(10))), (10, Failed)];
        polls.extend((20..=300).step_by(10).map(|now| (now, Failed)));
        polls.push((310, Press(at(50))));
        assert_eq!(
            edges(&mut contact, &polls),
            [
                Input::Touch(at(10)),
                Input::Cancel,
                Input::Release,
                Input::Touch(at(50))
            ]
        );

        // Input lost elsewhere with the finger down, then still sliding: its
        // presses stay quiet until it lifts.
        let mut contact = TouchContact::default();
        assert_eq!(
            contact.poll(Press(at(10)), 0),
            [Some(Input::Touch(at(10))), None, None]
        );
        assert!(contact.cancel(5));
        assert!(!contact.cancel(6), "no contact left to cancel");
        assert_eq!(contact.poll(Press(at(40)), 10), [None; 3]);
        assert_eq!(
            contact.poll(Release, 20),
            [Some(Input::Release), None, None]
        );
        assert_eq!(contact.poll(Release, 30), [None; 3]);
        assert_eq!(contact.poll(Nothing, 40), [None; 3]);
    }

    /// A finger the panel stops reporting ends at once, so a fresh tap right
    /// after the stale time is whole.
    #[test]
    fn a_stale_contact_ends_at_once() {
        let mut contact = TouchContact::default();
        let mut polls = std::vec![(0, Poll::Press(at(10)))];
        polls.extend((10..=260).step_by(10).map(|now| (now, Poll::Nothing)));
        polls.extend([(300, Poll::Press(at(50))), (310, Poll::Release)]);
        assert_eq!(
            edges(&mut contact, &polls),
            [
                Input::Touch(at(10)),
                Input::Cancel,
                Input::Release,
                Input::Touch(at(50)),
                Input::Release
            ]
        );
    }
}
