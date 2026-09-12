//! Position acquisition state. Publication replaces old state; consumers do not
//! acknowledge updates and cannot backpressure the receiver.
use crate::gps::{self, Parser};

/// Silence is a transport observation, distinct from a receiver reporting no fix.
pub const SILENT_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Unavailable,
    Receiving,
    Failed,
    Silent,
}
impl Transport {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Receiving => "receiving",
            Self::Failed => "failed",
            Self::Silent => "silent",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub gps: gps::Snapshot,
    pub transport: Transport,
    pub sequence: u32,
    pub received_at_ms: Option<u64>,
    pub fault_at_ms: Option<u64>,
}

#[derive(Clone, Default)]
pub struct Acquisition {
    parser: Parser,
    sequence: u32,
    dma_losses: u32,
    uart_errors: u32,
    last_received: Option<u64>,
    last_fault: Option<u64>,
    failed: bool,
}
impl Acquisition {
    /// Reset framing before bytes following a reported DMA/UART loss. Exactly
    /// the existing parser's loss, epoch and validity rules remain in force.
    pub fn ingest(&mut self, now: u64, bytes: &[u8], dma_losses: u32, uart_errors: u32) -> bool {
        let fault = dma_losses != self.dma_losses || uart_errors != self.uart_errors;
        self.parser
            .overflow(dma_losses.saturating_sub(self.dma_losses));
        if uart_errors != self.uart_errors {
            self.parser.data_loss();
        }
        self.dma_losses = dma_losses;
        self.uart_errors = uart_errors;
        if fault {
            self.last_fault = Some(now);
            self.failed = true;
        }
        if !bytes.is_empty() {
            self.last_received = Some(now);
            // A post-loss batch can be present in a fake/other transport. Keep
            // failure visible until a later clean receive batch proves recovery.
            if !fault {
                self.failed = false;
            }
            for &byte in bytes {
                self.parser.push(byte, now);
            }
        }
        let changed = fault || !bytes.is_empty();
        if changed {
            self.sequence = self.sequence.wrapping_add(1);
        }
        changed
    }

    /// Age is calculated at the caller's time, even without another publication.
    pub fn snapshot(&self, now: u64) -> Snapshot {
        let mut gps = self.parser.snapshot(now);
        gps.uart_errors = self.uart_errors;
        Snapshot {
            gps,
            transport: if self.failed {
                Transport::Failed
            } else {
                match self.last_received {
                    None => Transport::Unavailable,
                    Some(at) if now.saturating_sub(at) > SILENT_MS => Transport::Silent,
                    Some(_) => Transport::Receiving,
                }
            },
            sequence: self.sequence,
            received_at_ms: self.last_received,
            fault_at_ms: self.last_fault,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sentence(state: &mut Acquisition, body: &str, now: u64) {
        use std::format;
        let sum = body.bytes().fold(0, |sum, b| sum ^ b);
        let line = format!("${body}*{sum:02X}\r\n");
        state.ingest(now, line.as_bytes(), 0, 0);
    }
    #[test]
    fn readers_age_fix_and_satellites_without_publication() {
        let mut state = Acquisition::default();
        sentence(
            &mut state,
            "GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,",
            10,
        );
        let published = state.clone();
        assert_eq!(published.snapshot(11).gps.state, gps::FixState::Fresh);
        let stale = published.snapshot(3011);
        assert_eq!(stale.gps.state, gps::FixState::Stale);
        assert_eq!(stale.gps.satellites, None);
        assert_eq!(stale.transport, Transport::Silent);
        assert_eq!(stale.gps.age_ms, Some(3001));
    }
    #[test]
    fn no_fix_is_distinct_from_loss_and_clean_receive_recovers() {
        let mut state = Acquisition::default();
        assert_eq!(state.snapshot(0).transport, Transport::Unavailable);
        sentence(&mut state, "GPRMC,123519,V,,,,,,,230394,,,N", 10);
        assert_eq!(state.snapshot(10).gps.state, gps::FixState::NoFix);
        state.ingest(20, &[], 1, 2);
        let failed = state.snapshot(20);
        assert_eq!(failed.transport, Transport::Failed);
        assert_eq!((failed.gps.overflows, failed.gps.uart_errors), (1, 2));
        state.ingest(30, b"$", 1, 2);
        assert_eq!(state.snapshot(30).transport, Transport::Receiving);
        assert_eq!(state.snapshot(30).fault_at_ms, Some(20));
    }
    #[test]
    fn absent_consumer_does_not_retain_a_queue() {
        let mut state = Acquisition::default();
        for now in 0..1000 {
            state.ingest(now, b"noise", 0, 0);
        }
        assert_eq!(state.snapshot(1000).gps.bytes, 5000);
        assert_eq!(state.snapshot(1000).sequence, 1000);
        assert!(core::mem::size_of::<Acquisition>() < 1024);
    }
}
