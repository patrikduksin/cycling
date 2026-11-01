use crate::gps::Parser;
#[cfg(test)]
use device_api::gps;
use device_api::positioning::*;

/// A drain that found bytes is followed this soon: the rest of the epoch's
/// burst is still arriving, and readers see an epoch whole once all of it
/// is parsed.
pub const BURST_DRAIN_MS: u64 = 10;
/// Between bursts the receiver is drained this often, so a fix reaches
/// readers up to this late; its 1 Hz epochs, `SILENT_MS` and the parser's
/// clock tolerance are far coarser. The bytes of an epoch wait in the
/// receive buffer meanwhile, so one epoch's burst must fit in it.
pub const LIVE_DRAIN_MS: u64 = 100;
/// After this long without a byte or a stream boundary (a pause or resume)
/// the receiver is paused or gone. It is longer than the 5 s a resumed
/// stream may take to talk again.
pub const LISTEN_MS: u64 = 6_000;
/// A paused or gone receiver is drained this often.
pub const QUIET_DRAIN_MS: u64 = 500;

/// The rest before the next drain, after one that found `drained` bytes
/// `unheard_ms` after the receiver last sent a byte or its stream last
/// paused or resumed.
pub fn drain_rest_ms(drained: usize, unheard_ms: u64) -> u64 {
    if drained > 0 {
        BURST_DRAIN_MS
    } else if unheard_ms < LISTEN_MS {
        LIVE_DRAIN_MS
    } else {
        QUIET_DRAIN_MS
    }
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
    pub fn control_boundary(&mut self) {
        self.parser.control_boundary();
        self.sequence = self.sequence.wrapping_add(1);
    }
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

    /// The parser's latest framing excerpts, for the terminal.
    pub fn diagnostics(&self) -> device_api::gps::Diagnostics {
        self.parser.diagnostics()
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
    fn readers_get_receiver_utc_at_their_own_time_without_publication() {
        let mut state = Acquisition::default();
        sentence(
            &mut state,
            "GPRMC,100000.00,A,4807.038,N,01131.000,E,0,0,141026,,,A",
            5_000,
        );
        let published = state.clone();
        let at = |now| published.snapshot(now).gps.utc_ms;
        assert_eq!(at(6_000).unwrap() - at(5_000).unwrap(), 1_000);
        assert_eq!(at(60_000).unwrap() - at(5_000).unwrap(), 55_000);
        state.control_boundary();
        assert_eq!(state.snapshot(6_000).gps.utc_ms, None);
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
    /// Drains a receiver that sends a burst of `burst_ms` from `phase` into
    /// each second, at the rests `drain_rest_ms` picks, for `seconds`; the
    /// drain times and the bytes (ms of burst) each found.
    fn drains(phase: u64, burst_ms: u64, seconds: u64) -> std::vec::Vec<(u64, u64)> {
        let sent = |from: u64, to: u64| {
            (from..to)
                .filter(|ms| (phase..phase + burst_ms).contains(&(ms % 1000)))
                .count() as u64
        };
        let (mut out, mut now, mut last, mut heard) = (std::vec::Vec::new(), 0, 0, 0);
        while now < seconds * 1000 {
            let drained = sent(last, now);
            out.push((now, drained));
            if drained > 0 {
                heard = now;
            }
            last = now;
            now += drain_rest_ms(drained as usize, now - heard);
        }
        out
    }

    #[test]
    fn an_epoch_is_drained_soon_and_whole_and_the_quiet_receiver_rarely() {
        for phase in (100..900).step_by(7) {
            let drained = drains(phase, 30, 10);
            for second in 1..9 {
                let start = second * 1000 + phase;
                let found: std::vec::Vec<_> = drained
                    .iter()
                    .filter(|(at, ms)| *ms > 0 && (start..start + 1000).contains(at))
                    .collect();
                let (first, last) = (found[0].0, found[found.len() - 1].0);
                assert!(first - start <= LIVE_DRAIN_MS, "phase {phase}: seen late");
                assert!(
                    last <= (start + 30).max(first) + BURST_DRAIN_MS,
                    "phase {phase}: split"
                );
                assert_eq!(found.iter().map(|(_, ms)| ms).sum::<u64>(), 30);
            }
            let per_second = drained.iter().filter(|(at, _)| *at >= 1000).count() / 9;
            assert!(
                per_second <= 15,
                "phase {phase}: {per_second} drains a second"
            );
        }
        let silent = drains(0, 0, 20);
        let late = silent.iter().filter(|(at, _)| *at >= LISTEN_MS + 100);
        assert!(late.clone().count() as u64 <= (20_000 - LISTEN_MS) / QUIET_DRAIN_MS);
        assert!(late.clone().count() > 0);
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
