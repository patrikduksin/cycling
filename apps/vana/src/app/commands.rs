//! Application commands, completion reporting and ride export.
use super::Runtime;
use super::recording::Request;
use crate::ride::control::Action;
use crate::ride::log::{SLOT_SIZE, Slot, Source, VERSION, transport_checksum};
use crate::ride::recorder::Recorder;
use crate::ride::storage::RideStorage;
use core::fmt::Write;

/// Terminal replies older than this report no current sensor reading.
const SENSOR_REPLY_FRESH_MS: u64 = 3000;

impl Runtime {
    /// Single-line payload for the ordinary terminal's correlated reply.
    /// Mutations return ACCEPTED; RIDE STATUS reports the retained completion.
    pub fn command(
        &mut self,
        words: &str,
        store: &mut impl RideStorage,
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
            return self.submit(output, |recorder, token| recorder.clear(bound, token));
        }
        if bound.is_some() {
            return "INVALID";
        }
        if domain == Some("PROFILE") {
            return self.profile_command(operation, argument, now, output);
        }
        match (domain, operation, argument) {
            (Some("RIDE"), Some("INIT"), None) => {
                self.submit(output, |recorder, token| recorder.initialize(token))
            }
            (Some("RIDE"), Some("SENSORS"), None) => self.ble_sensors(now, output),
            (Some("RIDE"), Some("STATUS"), None) => self.ride_status(now, output),
            (Some("RIDE"), Some("HISTORY"), None) => self.ride_history(output),
            (Some("RIDE"), Some("TIMING"), None) => {
                self.timing.report(output);
                "OK"
            }
            (Some("RIDE"), Some(action @ ("START" | "PAUSE" | "RESUME" | "FINISH")), None) => {
                let action = match action {
                    "START" => Action::Start,
                    "PAUSE" => Action::Pause,
                    "RESUME" => Action::Resume,
                    _ => Action::Finish,
                };
                self.submit(output, |recorder, token| {
                    recorder.request(action, Source::Live, now, token)
                })
            }
            (Some("EXPORT"), Some("INFO"), None) => self.export_info(output),
            (Some("EXPORT"), Some("SLOT"), Some(index)) => self.export_slot(index, store, output),
            _ => "INVALID",
        }
    }

    /// Queue a recorder mutation, reporting its token before changing state so
    /// an undersized reply cannot hide that a command was queued.
    fn submit(
        &mut self,
        output: &mut impl Write,
        start: impl FnOnce(&mut Recorder, u32) -> bool,
    ) -> &'static str {
        if !matches!(self.ride.request, Request::Idle) {
            return "BUSY";
        }
        if write!(output, "token={}", self.ride.next_token()).is_err() {
            return "OUTPUT";
        }
        let token = self.ride.next_token();
        if !self.ride.submit(start) {
            return "STATE";
        }
        self.ride.host_token = Some(token);
        "ACCEPTED"
    }

    /// `PROFILE` reports the training profile; `PROFILE FTP|LTHR value` sets
    /// one threshold, saved once edits settle; `PROFILE OVERWRITE` replaces a
    /// stored profile this firmware cannot read.
    fn profile_command(
        &mut self,
        operation: Option<&str>,
        argument: Option<&str>,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        let mut profile = self.preferences.profile;
        match (operation, argument) {
            (None, None) => {}
            (Some(field @ ("FTP" | "LTHR")), Some(value)) => {
                let Ok(value) = value.parse::<u16>() else {
                    return "INVALID";
                };
                if field == "FTP" {
                    profile.ftp_watts = value;
                } else {
                    profile.lthr_bpm = value;
                }
                let Some(profile) =
                    crate::ride::zones::TrainingProfile::new(profile.ftp_watts, profile.lthr_bpm)
                else {
                    return "INVALID";
                };
                self.preferences.set(profile, now);
                self.display.redraw();
            }
            (Some("OVERWRITE"), None) => self.preferences.overwrite(now),
            _ => return "INVALID",
        }
        let preferences = &self.preferences;
        if write!(
            output,
            "ftp={} lthr={} stored={} persistence={}",
            preferences.profile.ftp_watts,
            preferences.profile.lthr_bpm,
            preferences.stored.name(),
            preferences.persistence().name()
        )
        .is_err()
        {
            return "OUTPUT";
        }
        "OK"
    }

    fn radar_command(
        &self,
        operation: Option<&str>,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        let result = match operation {
            None => write!(output, "{:?}", self.radar(now)),
            Some("SENSORS") => {
                let fresh =
                    |received_ms: u64| now.saturating_sub(received_ms) < SENSOR_REPLY_FRESH_MS;
                // Replies keep the established `(value, received_ms)` tuple format.
                let heart = self
                    .readings
                    .heart
                    .filter(|heart| fresh(heart.received_ms))
                    .map(|heart| (heart.value, heart.received_ms));
                let power = self
                    .readings
                    .power
                    .filter(|power| fresh(power.received_ms))
                    .map(|power| (power.value, power.received_ms));
                write!(output, "heart={:?} power={:?}", heart, power)
            }
            _ => return "INVALID",
        };
        if result.is_ok() { "OK" } else { "OUTPUT" }
    }

    fn ble_sensors(&self, now: u64, output: &mut impl Write) -> &'static str {
        let sensors = self.sensors.snapshot(now);
        if write!(
            output,
            "profile={} link={} heart={:?} heart_age_ms={:?} cadence={:?} cadence_age_ms={:?} invalid={} rr_dropped={}",
            sensors.profile.name(),
            sensors.link.name(),
            sensors.heart_bpm,
            sensors.heart_age_ms,
            sensors.cadence_tenths,
            sensors.cadence_age_ms,
            sensors.invalid,
            sensors.rr_dropped
        )
        .is_err()
        {
            return "OUTPUT";
        }
        "OK"
    }

    /// Reports and then clears a retained completion.
    fn ride_status(&mut self, now: u64, output: &mut impl Write) -> &'static str {
        let recorder = &self.ride.recorder;
        let pending = match self.ride.request {
            Request::Pending(token) => token,
            Request::Idle | Request::Completed(_) => 0,
        };
        if write!(
            output,
            "state={} slot={} rides={} active_ms={} samples={} dropped={} free_s={} page={} home_cursor={} pending={}",
            recorder.status().name(),
            recorder.next_slot(),
            recorder.completed(),
            recorder.active_ms(now),
            recorder.written_samples(),
            recorder.dropped_samples(),
            recorder.remaining_secs(),
            if self.power.view().is_some() {
                "power"
            } else {
                self.navigation.page.name()
            },
            self.navigation.home_cursor,
            pending
        )
        .is_err()
        {
            return "OUTPUT";
        }
        if let Request::Completed(result) = self.ride.request {
            let outcome = if result.ok { "OK" } else { "FAILED" };
            if write!(output, " completed={} result={outcome}", result.token).is_err() {
                return "OUTPUT";
            }
            self.ride.request = Request::Idle;
        }
        "OK"
    }

    fn ride_history(&self, output: &mut impl Write) -> &'static str {
        let recorder = &self.ride.recorder;
        if write!(output, "count={}", recorder.summary_count()).is_err() {
            return "OUTPUT";
        }
        for summary in recorder.summaries().iter().flatten() {
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

    fn export_info(&self, output: &mut impl Write) -> &'static str {
        let recorder = &self.ride.recorder;
        if !recorder.exportable() {
            return "BUSY";
        }
        if write!(
            output,
            "INFO {} {} {} {}",
            VERSION,
            SLOT_SIZE,
            recorder.next_slot(),
            recorder.status().name()
        )
        .is_err()
        {
            return "OUTPUT";
        }
        "OK"
    }

    fn export_slot(
        &self,
        index: &str,
        store: &mut impl RideStorage,
        output: &mut impl Write,
    ) -> &'static str {
        let recorder = &self.ride.recorder;
        if !recorder.exportable() {
            return "BUSY";
        }
        let Ok(index) = index.parse::<usize>() else {
            return "INVALID";
        };
        if index >= recorder.next_slot() {
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
            transport_checksum(&slot.0)
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
}
