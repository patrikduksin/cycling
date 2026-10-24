//! Application commands, completion reporting and ride export.
//!
//! `RIDE` and `EXPORT` take an optional medium, `BULK` or `INTERNAL`. Without
//! one, ride control and `RIDE STATUS`/`HISTORY` address the medium the rider's
//! ride uses, while `INIT`, `CLEAR` and `EXPORT` keep addressing the internal
//! journal, so existing tools never act on bulk media by accident.
use super::Runtime;
use super::recording::{Medium, Request};
use crate::ride::control::Action;
use crate::ride::log::{SLOT_SIZE, Slot, Source, VERSION, transport_checksum};
use crate::ride::recorder::Recorder;
use crate::ride::storage::RideStorage;
use core::fmt::Write;
use device_api::bulk::ReadWrite;

/// Terminal replies older than this report no current sensor reading.
const SENSOR_REPLY_FRESH_MS: u64 = 3000;

impl Runtime {
    /// Single-line payload for the ordinary terminal's correlated reply.
    /// Mutations return ACCEPTED; RIDE STATUS reports the retained completion.
    pub fn command(
        &mut self,
        words: &str,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        let mut workout = words.split_ascii_whitespace();
        if workout.next() == Some("WORKOUT") {
            return self.workout_command(&mut workout, bulk, now, output);
        }
        let mut words = words.split_ascii_whitespace().peekable();
        let domain = words.next();
        let named = match words.peek() {
            Some(&"BULK") => Some(Medium::Bulk),
            Some(&"INTERNAL") => Some(Medium::Internal),
            _ => None,
        };
        if named.is_some() {
            words.next();
        }
        let operation = words.next();
        let argument = words.next();
        let bound = words.next();
        if words.next().is_some() {
            return "INVALID";
        }
        if domain == Some("PHONE") {
            if named.is_some() || bound.is_some() {
                return "INVALID";
            }
            return self.phone_command(operation, argument, output);
        }
        if domain == Some("RADAR") {
            if named.is_some() || argument.is_some() || bound.is_some() {
                return "INVALID";
            }
            return self.radar_command(operation, now, output);
        }
        let stored = named.unwrap_or(Medium::Internal);
        if (domain, operation, argument) == (Some("RIDE"), Some("CLEAR"), Some("CONFIRM")) {
            let Some(bound) = bound.and_then(|value| value.parse::<usize>().ok()) else {
                return "INVALID";
            };
            if !self.verified_store(stored) {
                return "STATE";
            }
            return self.submit(stored, output, |recorder, token| {
                recorder.clear(bound, token)
            });
        }
        if bound.is_some() {
            return "INVALID";
        }
        if domain == Some("PROFILE") {
            if named.is_some() {
                return "INVALID";
            }
            return self.profile_command(operation, argument, now, output);
        }
        let riding = named.unwrap_or(self.ride.shown());
        match (domain, operation, argument) {
            (Some("RIDE"), Some("INIT"), None) => {
                self.submit(stored, output, |recorder, token| recorder.initialize(token))
            }
            (Some("RIDE"), Some("SENSORS"), None) if named.is_none() => {
                self.ble_sensors(now, output)
            }
            (Some("RIDE"), Some("STATUS"), None) => self.ride_status(riding, now, output),
            (Some("RIDE"), Some("HISTORY"), None) => self.ride_history(riding, output),
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
                if action == Action::Start && self.ride.open() {
                    // One ride at a time, whichever medium it is on.
                    return "STATE";
                }
                let medium = named.unwrap_or(self.ride.target(action));
                self.submit(medium, output, |recorder, token| {
                    recorder.request(action, Source::Live, now, token)
                })
            }
            (Some("EXPORT"), Some("INFO"), None) => self.export_info(stored, output),
            (Some("EXPORT"), Some("SLOT"), Some(index)) => {
                self.export_slot(stored, index, store, bulk, output)
            }
            _ => "INVALID",
        }
    }

    /// Queue a recorder mutation, reporting its token before changing state so
    /// an undersized reply cannot hide that a command was queued.
    fn submit(
        &mut self,
        medium: Medium,
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
        if !self.ride.submit(medium, start) {
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

    /// `PHONE` reports the phone session; `PHONE START [HOME]` opens one on
    /// the access point or the home network; `PHONE STOP` closes it. The
    /// reply carries the session's password and token: the terminal is the
    /// owner's.
    fn phone_command(
        &mut self,
        operation: Option<&str>,
        argument: Option<&str>,
        output: &mut impl Write,
    ) -> &'static str {
        match (operation, argument) {
            (None, None) => {}
            (Some("START"), None) => self.open_phone(super::PhoneMode::AccessPoint),
            (Some("START"), Some("HOME")) => self.open_phone(super::PhoneMode::Home),
            (Some("STOP"), None) => self.close_phone(),
            _ => return "INVALID",
        }
        let result = match self.phone_status() {
            Some(status) => {
                let url = status.url();
                write!(
                    output,
                    "state=open mode={} ssid={} password={} url={} ap={} requests={} downloads={}",
                    status.mode.name(),
                    status.ssid.text(),
                    status.password.text(),
                    url.as_ref().map_or("pending", |url| url.as_str()),
                    status.access_point.name(),
                    status.requests,
                    status.downloads
                )
            }
            None => write!(
                output,
                "state=closed reason={}",
                self.phone_closed().map_or("none", |closed| closed.name())
            ),
        };
        if result.is_err() {
            return "OUTPUT";
        }
        if operation.is_some() {
            "ACCEPTED"
        } else {
            "OK"
        }
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
    fn ride_status(&mut self, medium: Medium, now: u64, output: &mut impl Write) -> &'static str {
        let recorder = self.ride.recorder(medium);
        let pending = match self.ride.request {
            Request::Pending(token) => token,
            Request::Idle | Request::Completed(_) => 0,
        };
        if write!(
            output,
            "state={} slot={} rides={} active_ms={} samples={} dropped={} free_s={} medium={} write_ms={}",
            recorder.status().name(),
            recorder.next_slot(),
            recorder.completed(),
            recorder.active_ms(now),
            recorder.written_samples(),
            recorder.dropped_samples(),
            recorder.remaining_secs(),
            medium.name(),
            recorder.max_write_ms(),
        )
        .is_err()
        {
            return "OUTPUT";
        }
        if medium == Medium::Bulk {
            let log = self.ride.bulk_media.log();
            if write!(
                output,
                " capacity={} store={:016x} generation={} boot_reads={} layout={}",
                recorder.slots(),
                log.map_or(0, |log| log.store_id()),
                log.map_or(0, |log| log.generation()),
                self.ride.bulk_media.boot_reads(),
                self.ride.bulk_media.region().name()
            )
            .is_err()
            {
                return "OUTPUT";
            }
        }
        let fallback = self.ride.fallback();
        if write!(
            output,
            " fallback={} moved={} page={} home_cursor={} pending={}",
            u8::from(fallback),
            u8::from(self.ride.moved),
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

    fn ride_history(&self, medium: Medium, output: &mut impl Write) -> &'static str {
        let recorder = self.ride.recorder(medium);
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

    /// `INFO version slot_size upper state`; bulk adds its epoch identity and
    /// capacity so a host can tell epochs apart across a clear.
    /// Export and reclaim need a store whose extent is known: a bulk log
    /// without a recovered epoch (unformatted, or its metadata lost) has no
    /// verifiable upper bound, so it is neither empty nor clearable.
    fn verified_store(&self, medium: Medium) -> bool {
        medium == Medium::Internal || self.ride.bulk_media.log().is_some()
    }

    fn export_info(&self, medium: Medium, output: &mut impl Write) -> &'static str {
        let recorder = self.ride.recorder(medium);
        if !self.verified_store(medium) {
            return "STATE";
        }
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
        if medium == Medium::Bulk {
            let log = self.ride.bulk_media.log();
            if write!(
                output,
                " {:016x} {} {}",
                log.map_or(0, |log| log.store_id()),
                log.map_or(0, |log| log.generation()),
                recorder.slots()
            )
            .is_err()
            {
                return "OUTPUT";
            }
        }
        "OK"
    }

    /// `SLOT index crc hex`; bulk adds `valid` or `invalid` for the sector
    /// container, whose invalid slots are exported as zero bytes.
    fn export_slot(
        &mut self,
        medium: Medium,
        index: &str,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        output: &mut impl Write,
    ) -> &'static str {
        let recorder = self.ride.recorder(medium);
        if !self.verified_store(medium) {
            return "STATE";
        }
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
        let Some(valid) = self.ride.read_slot(medium, store, bulk, index, &mut slot) else {
            return "READ";
        };
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
        if medium == Medium::Bulk
            && write!(output, " {}", if valid { "valid" } else { "invalid" }).is_err()
        {
            return "OUTPUT";
        }
        "OK"
    }
}
