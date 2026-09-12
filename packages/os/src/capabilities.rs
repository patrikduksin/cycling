//! Presentation geometry and physical observations, without gesture policy.
use crate::{companion::Button, input::Point};

pub const PANEL_WIDTH: usize = 240;
pub const PANEL_HEIGHT: usize = 320;
pub const FRAME_WIDTH: usize = 80;
pub const FRAME_HEIGHT: usize = 106;
pub const FRAME_PIXELS: usize = FRAME_WIDTH * FRAME_HEIGHT;
pub const OBSERVATION_STALE_MS: u64 = 5_000;

/// Existing 3x mapping, including the panel's one-row top offset and bottom clamp.
/// Call only for coordinates within PANEL_WIDTH x PANEL_HEIGHT.
pub fn frame_index(x: usize, y: usize) -> usize {
    ((y.saturating_sub(1) / 3).min(FRAME_HEIGHT - 1)) * FRAME_WIDTH + x / 3
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation<T> {
    Unavailable,
    Fresh { value: T, received_ms: u64 },
    Stale { value: T, received_ms: u64 },
}
pub fn observation<T: Copy>(value: Option<(T, u64)>, now: u64) -> Observation<T> {
    match value {
        None => Observation::Unavailable,
        Some((value, received_ms)) if now.saturating_sub(received_ms) <= OBSERVATION_STALE_MS => {
            Observation::Fresh { value, received_ms }
        }
        Some((value, received_ms)) => Observation::Stale { value, received_ms },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Touch(Point),
    Release,
    /// Cancel all consumer-held input. Caused by invalid/lost touch or edge loss.
    Cancel,
    /// Known physical button with uninterpreted companion report code. Repeated
    /// code 1 reports do not establish a physical release; clients choose policy.
    Button {
        button: Button,
        code: u16,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub sequence: u32,
    pub received_ms: u64,
    pub input: Input,
}

/// Single-consumer bounded edge queue. On overflow discard queued history and
/// enqueue Cancel before the newest event, so consumers cannot retain old holds.
/// Acquisition never waits for a consumer. `lost` counts discarded input edges.
pub struct Edges {
    values: [Option<Edge>; 16],
    head: usize,
    len: usize,
    sequence: u32,
    pub lost: u32,
}
impl Default for Edges {
    fn default() -> Self {
        Self::new()
    }
}
impl Edges {
    pub const fn new() -> Self {
        Self {
            values: [None; 16],
            head: 0,
            len: 0,
            sequence: 0,
            lost: 0,
        }
    }
    pub fn push(&mut self, received_ms: u64, input: Input) {
        if self.len == self.values.len() {
            self.lost = self.lost.saturating_add(self.len as u32);
            self.head = 0;
            self.len = 0;
            self.insert(received_ms, Input::Cancel);
        }
        self.insert(received_ms, input);
    }
    fn insert(&mut self, received_ms: u64, input: Input) {
        self.sequence = self.sequence.wrapping_add(1);
        let index = (self.head + self.len) % self.values.len();
        self.values[index] = Some(Edge {
            sequence: self.sequence,
            received_ms,
            input,
        });
        self.len += 1;
    }
    pub fn pop(&mut self) -> Option<Edge> {
        if self.len == 0 {
            return None;
        }
        let value = self.values[self.head].take();
        self.head = (self.head + 1) % self.values.len();
        self.len -= 1;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slow_consumer_gets_cancel_before_new_input() {
        let mut edges = Edges::new();
        for t in 0..16 {
            edges.push(t, Input::Touch(Point { x: t as u16, y: 0 }));
        }
        edges.push(16, Input::Release);
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
            edges.push(t, Input::Release);
            assert_eq!(edges.pop().unwrap().received_ms, t);
        }
        assert_eq!(edges.lost, 0);
    }
    #[test]
    fn stale_values_keep_observation_time_and_unavailable_stays_absent() {
        assert_eq!(observation::<u8>(None, 9000), Observation::Unavailable);
        assert_eq!(
            observation(Some((50, 10)), 5011),
            Observation::Stale {
                value: 50,
                received_ms: 10
            }
        );
        assert_eq!(
            observation(Some((50, 10)), 5010),
            Observation::Fresh {
                value: 50,
                received_ms: 10
            }
        );
    }
    #[test]
    fn every_panel_pixel_maps_inside_frame_and_matches_existing_edges() {
        for y in 0..PANEL_HEIGHT {
            for x in 0..PANEL_WIDTH {
                assert!(frame_index(x, y) < FRAME_PIXELS);
            }
        }
        assert_eq!(frame_index(0, 0), 0);
        assert_eq!(frame_index(0, 3), 0);
        assert_eq!(frame_index(0, 4), FRAME_WIDTH);
        assert_eq!(frame_index(239, 319), FRAME_PIXELS - 1);
    }
}
