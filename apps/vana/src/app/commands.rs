//! Application commands, completion reporting and ride export.
use super::Runtime;
use crate::ride::{
    control::Action,
    log::{Slot, Source},
};
use core::fmt::Write;

impl Runtime {
    fn radar_command(
        &self,
        operation: Option<&str>,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        let result = match operation {
            None => write!(output, "{:?}", self.radar(now)),
            Some("SENSORS") => {
                let heart = self.heart.filter(|(_, at)| now.saturating_sub(*at) < 3000);
                let power = self.power.filter(|(_, at)| now.saturating_sub(*at) < 3000);
                write!(output, "heart={:?} power={:?}", heart, power)
            }
            _ => return "INVALID",
        };
        if result.is_ok() { "OK" } else { "OUTPUT" }
    }

    /// Single-line payload for the ordinary terminal's correlated reply.
    /// Mutations return ACCEPTED; RIDE STATUS reports the retained completion.
    pub fn command(
        &mut self,
        words: &str,
        store: &mut impl crate::ride::storage::RideStorage,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        let mut words = words.split_ascii_whitespace();
        let domain = words.next();
        let operation = words.next();
        let argument = words.next();
        let bound = words.next();
        if words.next().is_some() {
            return "INVALID";
        }
        if domain == Some("RADAR") {
            if argument.is_some() || bound.is_some() {
                return "INVALID";
            }
            return self.radar_command(operation, now, output);
        }
        if (domain, operation, argument) == (Some("RIDE"), Some("CLEAR"), Some("CONFIRM")) {
            let Some(bound) = bound.and_then(|value| value.parse::<usize>().ok()) else {
                return "INVALID";
            };
            if self.pending.is_some() || self.completion.is_some() {
                return "BUSY";
            }
            let token = self.next_token;
            if write!(output, "token={token}").is_err() {
                return "OUTPUT";
            }
            if !self.recorder.clear(bound, token) {
                return "STATE";
            }
            self.next_token = self.next_token.wrapping_add(1).max(1);
            self.pending = Some(token);
            return "ACCEPTED";
        }
        if bound.is_some() {
            return "INVALID";
        }
        match (domain, operation, argument) {
            (Some("RIDE"), Some("INIT"), None) => {
                if self.pending.is_some() || self.completion.is_some() {
                    return "BUSY";
                }
                let token = self.next_token;
                if write!(output, "token={token}").is_err() {
                    return "OUTPUT";
                }
                if !self.recorder.initialize(token) {
                    return "STATE";
                }
                self.next_token = self.next_token.wrapping_add(1).max(1);
                self.pending = Some(token);
                "ACCEPTED"
            }
            (Some("RIDE"), Some("SENSORS"), None) => {
                let sensors = self.sensors.snapshot(now);
                if write!(output, "profile={} link={} heart={:?} heart_age_ms={:?} cadence={:?} cadence_age_ms={:?} invalid={} rr_dropped={}", sensors.profile.name(), sensors.link.name(), sensors.heart_bpm, sensors.heart_age_ms, sensors.cadence_tenths, sensors.cadence_age_ms, sensors.invalid, sensors.rr_dropped).is_err() { return "OUTPUT"; }
                "OK"
            }
            (Some("RIDE"), Some("STATUS"), None) => {
                if write!(
                    output,
                    "state={} slot={} rides={} active_ms={} samples={} dropped={} pending={}",
                    self.recorder.status().name(),
                    self.recorder.next_slot(),
                    self.recorder.completed(),
                    self.recorder.active_ms(now),
                    self.recorder.written_samples(),
                    self.recorder.dropped_samples(),
                    self.pending.unwrap_or(0)
                )
                .is_err()
                {
                    return "OUTPUT";
                }
                if let Some(result) = self.completion {
                    if write!(
                        output,
                        " completed={} result={}",
                        result.token,
                        if result.ok { "OK" } else { "FAILED" }
                    )
                    .is_err()
                    {
                        return "OUTPUT";
                    }
                    self.completion = None;
                }
                "OK"
            }
            (Some("RIDE"), Some("HISTORY"), None) => {
                if write!(output, "count={}", self.recorder.summary_count()).is_err() {
                    return "OUTPUT";
                }
                for summary in self.recorder.summaries().iter().flatten() {
                    if write!(
                        output,
                        " ride={},{},{},{},{},{}",
                        summary.ride_id,
                        summary.source.map(Source::name).unwrap_or("unknown"),
                        summary.active_ms,
                        u8::from(summary.recovered),
                        u8::from(summary.full),
                        u8::from(summary.gap)
                    )
                    .is_err()
                    {
                        return "OUTPUT";
                    }
                }
                "OK"
            }
            (Some("RIDE"), Some(action @ ("START" | "PAUSE" | "RESUME" | "FINISH")), None) => {
                if self.pending.is_some() || self.completion.is_some() {
                    return "BUSY";
                }
                let action = match action {
                    "START" => Action::Start,
                    "PAUSE" => Action::Pause,
                    "RESUME" => Action::Resume,
                    _ => Action::Finish,
                };
                let token = self.next_token;
                // Serialize the acceptance before changing state so an undersized
                // reply cannot hide that a command was queued.
                if write!(output, "token={token}").is_err() {
                    return "OUTPUT";
                }
                if !self.recorder.request(action, Source::Live, now, token) {
                    return "STATE";
                }
                self.next_token = self.next_token.wrapping_add(1).max(1);
                self.pending = Some(token);
                "ACCEPTED"
            }
            (Some("EXPORT"), Some("INFO"), None) => {
                if !self.recorder.exportable() {
                    return "BUSY";
                }
                if write!(
                    output,
                    "INFO {} {} {} {}",
                    crate::ride::log::VERSION,
                    crate::ride::log::SLOT_SIZE,
                    self.recorder.next_slot(),
                    self.recorder.status().name()
                )
                .is_err()
                {
                    return "OUTPUT";
                }
                "OK"
            }
            (Some("EXPORT"), Some("SLOT"), Some(index)) => {
                if !self.recorder.exportable() {
                    return "BUSY";
                }
                let Ok(index) = index.parse::<usize>() else {
                    return "INVALID";
                };
                if index >= self.recorder.next_slot() {
                    return "BOUNDS";
                }
                let mut slot = Slot::default();
                if store.ride_read_slot(index, &mut slot).is_err() {
                    return "READ";
                }
                if write!(
                    output,
                    "SLOT {} {:08x} ",
                    index,
                    crate::ride::log::transport_checksum(&slot.0)
                )
                .is_err()
                {
                    return "OUTPUT";
                }
                for byte in slot.0 {
                    if write!(output, "{byte:02x}").is_err() {
                        return "OUTPUT";
                    }
                }
                "OK"
            }
            _ => "INVALID",
        }
    }
}
