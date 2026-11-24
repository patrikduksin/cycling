//! Application commands, completion reporting and ride export.
//!
//! `RIDE` and `EXPORT` take an optional medium, `BULK` or `INTERNAL`. Without
//! one, ride control and `RIDE STATUS`/`HISTORY` address the medium the rider's
//! ride uses, while `INIT`, `CLEAR` and `EXPORT` keep addressing the internal
//! journal, so existing tools never act on bulk media by accident.
use super::Runtime;
use super::pace::Touched;
use crate::ride::current::{Op, Request};
use crate::ride::journal::Read;
use crate::ride::log::{SLOT_SIZE, Slot, VERSION, transport_checksum};
use crate::ride::medium::Medium;
use crate::ride::recorder::{Action, Refused};
use crate::ride::storage::RideStorage;
use crate::sensors::ble_profile::Profile;
use core::fmt::Write;
use device_api::ble_transport::Selection;
use device_api::bulk::ReadWrite;
use device_api::display::Display;
use device_api::input::InputSource;
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_shell::shell::Shell;

/// Terminal replies older than this report no current sensor reading.
const SENSOR_REPLY_FRESH_MS: u64 = 3000;

#[derive(Clone, Copy)]
pub(super) enum NotQueued {
    Busy,
    Output,
    Foreign,
    State,
}

impl NotQueued {
    fn reply(queued: Result<(), Self>) -> &'static str {
        match queued {
            Ok(()) => "ACCEPTED",
            Err(Self::Busy) => "BUSY",
            Err(Self::Output) => "OUTPUT",
            Err(Self::Foreign) => "FOREIGN",
            Err(Self::State) => "STATE",
        }
    }
}

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
        // Wrapped as `tick` wraps them, so the storage code both reach is
        // built once.
        let (store, bulk) = (&mut Touched::new(store), &mut Touched::new(bulk));
        if let Some(argument) = words.strip_prefix("FORMAT") {
            return self.format_command(argument.trim(), output);
        }
        let mut parts = words.split_ascii_whitespace().peekable();
        let domain = parts.next();
        let named = match parts.peek() {
            Some(&"BULK") => Some(Medium::Bulk),
            Some(&"INTERNAL") => Some(Medium::Internal),
            _ => None,
        };
        if named.is_some() {
            parts.next();
        }
        let operation = parts.next();
        let argument = parts.next();
        let bound = parts.next();
        let more = parts.next().is_some();
        // `RIDE [BULK|INTERNAL] STATUS` reads no storage, so a host
        // following a Clear reads it while the wipe holds storage.
        let status = (domain, operation, argument, bound, more)
            == (Some("RIDE"), Some("STATUS"), None, None, false);
        if self.format.holds_host_storage() && !status {
            return "BUSY";
        }
        let mut workout = words.split_ascii_whitespace();
        if workout.next() == Some("WORKOUT") {
            return self.workout_command(&mut workout, bulk, output);
        }
        if more {
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
            if stored == Medium::Internal {
                return NotQueued::reply(self.clear_internal(bound, output).map(drop));
            }
            if !self.verified_store(stored) {
                return "STATE";
            }
            if !self.format.admits_rides() {
                return "BUSY";
            }
            return self.submit(stored, output, Op::Clear(bound));
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
        if domain == Some("OPTION") {
            if named.is_some() {
                return "INVALID";
            }
            return self.option_command(operation, argument, now, output);
        }
        let riding = named.unwrap_or(self.ride.shown());
        match (domain, operation, argument) {
            (Some("RIDE"), Some("INIT"), None) => self.submit(stored, output, Op::Initialize),
            (Some("RIDE"), Some("SENSORS"), None) if named.is_none() => {
                self.ble_sensors(now, output)
            }
            (Some("RIDE"), Some("STATUS"), None) => self.ride_status(riding, now, output),
            (Some("RIDE"), Some("HISTORY"), None) => self.ride_history(riding, output),
            (Some("RIDE"), Some("WHEEL"), None) if named.is_none() => {
                match self.wheel_status(output) {
                    Ok(()) => "OK",
                    Err(_) => "OUTPUT",
                }
            }
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
                if !self.format.admits_rides() {
                    return "BUSY";
                }
                let medium = named.unwrap_or(self.ride.target(action));
                self.submit(medium, output, Op::Request(action, now))
            }
            (Some("EXPORT"), Some("INFO"), None) => self.export_info(stored, output),
            (Some("EXPORT"), Some("SLOT"), Some(index)) => {
                self.export_slot(stored, index, store, bulk, output)
            }
            _ => "INVALID",
        }
    }

    /// `RIDE CLEAR CONFIRM` of the internal journal, from the host or the
    /// phone page: the token it was queued under.
    pub(super) fn clear_internal(
        &mut self,
        bound: usize,
        output: &mut impl Write,
    ) -> Result<u32, NotQueued> {
        if !self.format.admits_rides() {
            return Err(NotQueued::Busy);
        }
        if self.ride.open() {
            return Err(NotQueued::State);
        }
        let clear = self.queue(Medium::Internal, output, Op::Clearable(bound))?;
        self.clear_internal_rides(clear);
        Ok(clear)
    }

    /// Queue a recorder mutation, reporting its token before changing state so
    /// an undersized reply cannot hide that a command was queued. The host's
    /// previous outcome must be read first; a rider's never holds it up.
    fn submit(&mut self, medium: Medium, output: &mut impl Write, op: Op) -> &'static str {
        NotQueued::reply(self.queue(medium, output, op).map(drop))
    }

    fn queue(&mut self, medium: Medium, output: &mut impl Write, op: Op) -> Result<u32, NotQueued> {
        if !matches!(self.ride.host(), Request::Idle) || self.ride.pending() {
            return Err(NotQueued::Busy);
        }
        if write!(output, "token={}", self.ride.next_token()).is_err() {
            return Err(NotQueued::Output);
        }
        self.ride
            .submit_host(medium, op)
            .map_err(|refused| match refused {
                Refused::Foreign => NotQueued::Foreign,
                Refused::State => NotQueued::State,
            })
    }

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
            (Some(field @ ("WEIGHT" | "AGE" | "SEX")), Some(value)) => {
                let mut body = self.preferences.options.rider.encode();
                let parsed = match (field, value) {
                    ("SEX", "M") => Some((2, 1)),
                    ("SEX", "F") => Some((2, 2)),
                    ("SEX", "-") => Some((2, 0)),
                    ("SEX", _) => None,
                    ("WEIGHT", value) => value.parse().ok().map(|kg| (0, kg)),
                    (_, value) => value.parse().ok().map(|years| (1, years)),
                };
                let Some((index, byte)) = parsed else {
                    return "INVALID";
                };
                body[index] = byte;
                let Some(rider) = crate::ride::rider::Rider::decode(body) else {
                    return "INVALID";
                };
                let mut options = self.preferences.options;
                options.rider = rider;
                self.preferences.set_options(options, now);
            }
            _ => return "INVALID",
        }
        let preferences = &self.preferences;
        let [weight, age, sex] = preferences.options.rider.encode();
        if write!(
            output,
            "ftp={} lthr={} weight={weight} age={age} sex={} stored={} persistence={}",
            preferences.profile.ftp_watts,
            preferences.profile.lthr_bpm,
            ["-", "m", "f"][usize::from(sex)],
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

    /// Reports the queued operation of either actor, then reports and clears
    /// the host's own retained completion.
    fn ride_status(&mut self, medium: Medium, now: u64, output: &mut impl Write) -> &'static str {
        let recorder = self.ride.recorder(medium);
        let pending = self.ride.pending_token().unwrap_or(0);
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
            let log = self.ride.bulk_media().log();
            if write!(
                output,
                " capacity={} store={:016x} generation={} boot_reads={} layout={}",
                recorder.slots().unwrap_or(0),
                log.map_or(0, |log| log.store_id()),
                log.map_or(0, |log| log.generation()),
                self.ride.bulk_media().boot_reads(),
                self.ride.bulk_media().region().name()
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
            u8::from(self.ride.moved()),
            if self.power.view().is_some() {
                "power"
            } else {
                self.navigation.page().name()
            },
            self.focus_index(),
            pending
        )
        .is_err()
        {
            return "OUTPUT";
        }
        let completed = match self.ride.host() {
            Request::Completed(result) => Some((result.token, result.ok)),
            Request::Declined(token) => Some((token, false)),
            Request::Idle | Request::Pending(_) => None,
        };
        if let Some((token, ok)) = completed {
            let outcome = if ok { "OK" } else { "FAILED" };
            if write!(output, " completed={token} result={outcome}").is_err() {
                return "OUTPUT";
            }
            self.ride.host_reported();
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
                " ride={},{},{},{},{}",
                summary.ride_id,
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
        medium == Medium::Internal || self.ride.bulk_media().log().is_some()
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
            let log = self.ride.bulk_media().log();
            if write!(
                output,
                " {:016x} {} {}",
                log.map_or(0, |log| log.store_id()),
                log.map_or(0, |log| log.generation()),
                recorder.slots().unwrap_or(0)
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
        let valid = match self.ride.read_slot(medium, store, bulk, index, &mut slot) {
            Read::Slot => true,
            Read::Invalid => false,
            Read::Failed => return "READ",
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

impl Runtime {
    /// BLE SELECT, or FORGET when `choice` is none. On success the caller asks
    /// its radio for the returned selection.
    pub fn select_ble<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        shell: &mut Shell<D, I, P, B>,
        radio: device_api::connectivity::Operation,
        connections: u32,
        choice: Option<(Selection, u8)>,
    ) -> Result<Option<Selection>, &'static str> {
        if radio == device_api::connectivity::Operation::Pending {
            return Err("UNAVAILABLE");
        }
        let (selection, profile) = match choice {
            Some((peer, profile)) => {
                let profile = Profile::from_u8(profile);
                (
                    crate::sensors::ble::selection(profile, peer.name.bytes(), peer.address),
                    profile,
                )
            }
            None => (None, Profile::Echo),
        };
        let previous = shell.settings;
        shell.settings.ble = selection;
        shell.settings.ble_profile = profile as u8;
        if !shell.save_connectivity() {
            shell.settings = previous;
            return Err("FAILED");
        }
        self.select_profile(profile, connections);
        Ok(selection)
    }
}
