//! Bounded physical input delivery with explicit cancellation on overflow.
use device_api::input::Edge;
use device_api::input::Input;
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
    use device_api::input::Point;
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
}
