//! Fixed JSON Lines records. Encoding failures and queue saturation drop whole records.
use serde::Serialize;

pub const RECORD_BYTES: usize = 384;
pub const QUEUE_SLOTS: usize = 8;

#[derive(Clone, Copy)]
pub struct Line {
    pub bytes: [u8; RECORD_BYTES],
    pub len: usize,
}
impl Line {
    const EMPTY: Self = Self {
        bytes: [0; RECORD_BYTES],
        len: 0,
    };
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Serialize)]
pub struct Record<'a> {
    pub r#type: &'static str,
    pub boot: u32,
    pub seq: u32,
    pub ms: u64,
    pub level: &'a str,
    pub component: &'a str,
    pub event: &'a str,
    pub message: &'a str,
    pub lost: u32,
}

pub struct Queue {
    lines: [Line; QUEUE_SLOTS],
    head: usize,
    count: usize,
    seq: u32,
    lost: u32,
}
impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}
impl Queue {
    pub const fn new() -> Self {
        Self {
            lines: [Line::EMPTY; QUEUE_SLOTS],
            head: 0,
            count: 0,
            seq: 0,
            lost: 0,
        }
    }
    pub fn push(&mut self, mut record: Record<'_>) {
        record.seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        record.lost = self.lost;
        if self.count == QUEUE_SLOTS {
            self.lose();
            return;
        }
        let line = &mut self.lines[(self.head + self.count) % QUEUE_SLOTS];
        match serde_json_core::to_slice(&record, &mut line.bytes[..RECORD_BYTES - 1]) {
            Ok(len) => {
                line.bytes[len] = b'\n';
                line.len = len + 1;
                self.count += 1;
            }
            Err(_) => self.lose(),
        }
    }
    fn lose(&mut self) {
        self.lost = self.lost.saturating_add(1);
    }
    pub fn lost(&self) -> u32 {
        self.lost
    }
    pub fn depth(&self) -> usize {
        self.count
    }
    pub fn pop(&mut self) -> Option<Line> {
        if self.count == 0 {
            return None;
        }
        let line = self.lines[self.head];
        self.head = (self.head + 1) % QUEUE_SLOTS;
        self.count -= 1;
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(message: &str) -> Record<'_> {
        Record {
            r#type: "log",
            boot: 1,
            seq: 0,
            ms: 2,
            level: "INFO",
            component: "test",
            event: "message",
            message,
            lost: 0,
        }
    }
    #[test]
    fn overflow_preserves_old_records_and_reports_loss() {
        let mut q = Queue::new();
        for _ in 0..10 {
            q.push(record("ok"));
        }
        for _ in 0..8 {
            assert!(q.pop().is_some());
        }
        assert!(q.pop().is_none());
        q.push(record("next"));
        let l = q.pop().unwrap();
        let s = core::str::from_utf8(l.as_bytes()).unwrap();
        assert!(s.contains("\"seq\":10"));
        assert!(s.contains("\"lost\":2"));
    }
    #[test]
    fn encoder_escapes_and_never_emits_partial_record() {
        let mut q = Queue::new();
        q.push(record("\"\n\\"));
        let l = q.pop().unwrap();
        assert!(
            core::str::from_utf8(l.as_bytes())
                .unwrap()
                .contains("\\\"\\n\\\\")
        );
        q.push(record(&"x".repeat(1000)));
        assert!(q.pop().is_none());
        q.push(record("fine"));
        let l = q.pop().unwrap();
        assert!(
            core::str::from_utf8(l.as_bytes())
                .unwrap()
                .contains("\"lost\":1")
        );
    }
    #[test]
    fn sequence_wraps() {
        let mut q = Queue::new();
        q.seq = u32::MAX;
        q.push(record("a"));
        q.push(record("b"));
        q.pop();
        let l = q.pop().unwrap();
        assert!(
            core::str::from_utf8(l.as_bytes())
                .unwrap()
                .contains("\"seq\":0")
        );
    }
}
