//! Composition of core snapshots and the portable cycling SDK.
//! Main calls tick independently of terminal requests. Flash work remains
//! synchronous and bounded to one recorder step, with no lock held across it.
use core::fmt::Write;
use cycling_os::{
    capabilities::Observation,
    gps::FixState,
    sdk::{
        ble::Client,
        ble_sensor::{self, Profile},
        recorder::{Recorder, ResultEvent},
        ride::Action,
        ride_log::{self, Sample, Slot, Source},
        storage::RideStorage,
    },
};

pub struct Runtime {
    radar: cycling_os::sdk::radar::Radar,
    capture: cycling_os::sdk::ant_capture::Capturer,
    capture_started: Option<u64>,
    next_display: u64,
    next_reconnect: u64,
    capture_link: Option<(cycling_os::ant::LinkState, u32)>,
    ant_generation: u32,
    ant_losses: u32,
    recorder: Recorder,
    sensors: Client,
    next_token: u32,
    pending: Option<u32>,
    completion: Option<ResultEvent>,
}

impl Runtime {
    pub fn new(profile: Profile) -> Self {
        Self {
            radar: cycling_os::sdk::radar::Radar::new(),
            capture: cycling_os::sdk::ant_capture::Capturer::new(),
            capture_started: None,
            next_display: 0,
            next_reconnect: 0,
            capture_link: None,
            ant_generation: 0,
            ant_losses: 0,
            recorder: Recorder::default(),
            sensors: Client::new(profile),
            next_token: 1,
            pending: None,
            completion: None,
        }
    }

    pub fn radar(&self, now: u64) -> cycling_os::sdk::radar::Snapshot {
        self.radar.snapshot(now)
    }

    pub fn test_display(&mut self, system: &mut crate::core_system::System, now: u64) {
        let Some(started) = self.capture_started else {
            return;
        };
        if now < self.next_display {
            return;
        }
        self.next_display = now.saturating_add(1000);
        let ant = crate::services::ant::snapshot(now);
        let capture = self.capture.snapshot();
        let screen = cycling_os::sdk::radar_screen::Screen {
            connected: ant.link == cycling_os::ant::LinkState::Connected && !ant.stale,
            logging: capture.recording
                && capture.packets > 0
                && capture
                    .last_commit_ms
                    .is_some_and(|at| now.saturating_sub(at) < 3000),
            saved_packets: capture.packets,
            dropped: capture
                .dropped
                .saturating_add(capture.dropped_links)
                .saturating_add(ant.dropped_packets),
            elapsed_secs: (now.saturating_sub(started) / 1000).min(u64::from(u32::MAX)) as u32,
            error: capture.error.is_some()
                || capture.status == cycling_os::sdk::ant_capture::Status::Full,
        };
        system.activity(now);
        system.draw_pixels(|x, y| cycling_os::sdk::radar_screen::pixel(x, y, screen));
    }

    fn radar_command(
        &mut self,
        operation: Option<&str>,
        argument: Option<&str>,
        bound: Option<&str>,
        store: &mut cycling_os::storage::Store<crate::device::storage::Backend<'static>>,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        use cycling_os::sdk::ant_capture::Status as CaptureStatus;
        if operation.is_none() && argument.is_none() && bound.is_none() {
            let _ = write!(output, "{:?}", self.radar(now));
            return "OK";
        }
        if operation != Some("LOG") {
            return "INVALID";
        }
        match (argument, bound) {
            (Some("START"), None) => {
                if self.capture_started.is_some()
                    || self.pending.is_some()
                    || self.completion.is_some()
                    || !self.recorder.exportable()
                {
                    return "BUSY";
                }
                let ant = crate::services::ant::snapshot(now);
                if ant.link != cycling_os::ant::LinkState::Connected
                    || ant.stale
                    || !ant.selected.is_some_and(|peer| peer.device_type == 40)
                {
                    return "RADAR_NOT_READY";
                }
                if !self.capture.start() {
                    return "STATE";
                }
                self.capture_started = Some(now);
                "ACCEPTED"
            }
            (Some("STOP"), None) => {
                self.capture.stop();
                "ACCEPTED"
            }
            (Some("STATUS"), None) => {
                let _ = write!(output, "{:?}", self.capture.snapshot());
                "OK"
            }
            (Some("INFO"), None) | (Some("READ"), Some(_)) => {
                let capture = self.capture.snapshot();
                if matches!(
                    capture.status,
                    CaptureStatus::Scanning
                        | CaptureStatus::Ready
                        | CaptureStatus::Recording
                        | CaptureStatus::Stopping
                ) || (self.capture_started.is_none() && !self.recorder.exportable())
                {
                    return "BUSY";
                }
                let upper = if self.capture_started.is_some() {
                    if capture.error.is_some() || capture.scanned < ride_log::SECTORS {
                        ride_log::SLOTS
                    } else {
                        capture.next_slot
                    }
                } else {
                    self.recorder.next_slot()
                };
                if argument == Some("INFO") {
                    let _ = write!(output, "INFO 1 256 {} {}", upper, capture.status.name());
                } else {
                    let Some(index) = bound.and_then(|v| v.parse::<usize>().ok()) else {
                        return "INVALID";
                    };
                    if index >= upper {
                        return "BOUNDS";
                    }
                    let mut slot = Slot::default();
                    if store.ride_read_slot(index, &mut slot).is_err() {
                        return "READ";
                    }
                    let _ = write!(
                        output,
                        "SLOT {} {:08x} ",
                        index,
                        ride_log::transport_checksum(&slot.0)
                    );
                    for byte in slot.0 {
                        let _ = write!(output, "{byte:02x}");
                    }
                }
                "OK"
            }
            _ => "INVALID",
        }
    }

    pub fn tick(
        &mut self,
        store: &mut cycling_os::storage::Store<crate::device::storage::Backend<'static>>,
        now: u64,
    ) {
        let ant = crate::services::ant::snapshot(now);
        if self.capture_started.is_some() {
            if matches!(
                self.capture.snapshot().status,
                cycling_os::sdk::ant_capture::Status::Ready
                    | cycling_os::sdk::ant_capture::Status::Recording
            ) && self.capture_link != Some((ant.link, ant.generation))
            {
                self.capture.observe_link(ant.link, ant.generation, now);
                self.capture_link = Some((ant.link, ant.generation));
            }
            if matches!(
                self.capture.snapshot().status,
                cycling_os::sdk::ant_capture::Status::Scanning
                    | cycling_os::sdk::ant_capture::Status::Ready
                    | cycling_os::sdk::ant_capture::Status::Recording
            ) && ant.link == cycling_os::ant::LinkState::Disconnected
                && now >= self.next_reconnect
            {
                if let Some(peer) = ant.selected {
                    let _ = crate::services::ant::request(
                        crate::services::ant::Operation::Connect(peer),
                        now,
                    );
                    self.next_reconnect = now.saturating_add(5000);
                }
            }
        }
        if ant.generation != self.ant_generation
            || ant.link != cycling_os::ant::LinkState::Connected
        {
            self.radar.reset();
            self.ant_generation = ant.generation;
        }
        for _ in 0..cycling_os::ant::PACKET_CAPACITY {
            let Some(packet) = crate::services::ant::take_packet() else {
                break;
            };
            if packet.generation != self.ant_generation || packet.loss_count != self.ant_losses {
                self.radar.reset();
                self.ant_generation = packet.generation;
                self.ant_losses = packet.loss_count;
            }
            self.capture.packet(packet);
            if packet.identity.device_type == 40 {
                self.radar.receive(packet.data, packet.received_ms);
            }
        }
        let transport = crate::bluetooth::snapshot();
        self.sensors.update(transport);
        // Core queue has two packets. Never drain an unbounded producer here.
        for _ in 0..2 {
            let Some(packet) = crate::bluetooth::take_packet() else {
                break;
            };
            self.sensors.packet(packet);
        }
        let sensors = self.sensors.snapshot(now);
        let (heart_bpm, cadence_tenths) = ble_sensor::ride_fields(sensors, self.recorder.source());
        let location_e7 = crate::services::positioning::snapshot(now).and_then(|snapshot| {
            (snapshot.gps.state == FixState::Fresh)
                .then(|| Some((snapshot.gps.latitude_e7?, snapshot.gps.longitude_e7?)))
                .flatten()
        });
        let clock = cycling_os::network_time::snapshot(now, 0, crate::wifi::online());
        let utc_ms = clock.unix_seconds.and_then(|seconds| {
            seconds
                .checked_mul(1_000)?
                .checked_add(u64::from(clock.millis))
        });
        // Keep existing last-observed battery behavior. Core status still exposes
        // its age; these ride format bytes have no battery-age field.
        let battery_percent =
            crate::services::io::snapshot(now).and_then(|snapshot| match snapshot.battery {
                Observation::Fresh {
                    value: (percent, _),
                    ..
                }
                | Observation::Stale {
                    value: (percent, _),
                    ..
                } => Some(percent),
                Observation::Unavailable => None,
            });
        let sample = Sample {
            active_ms: 0,
            utc_ms,
            location_e7,
            demo_speed_mm_s: None,
            heart_bpm,
            cadence_tenths,
            battery_percent,
        };
        if self.capture_started.is_some() {
            self.capture.service(store, now);
        } else {
            self.recorder.service(store, now, sample, &|| {
                embassy_time::Instant::now().as_millis()
            });
        }
        if let Some(result) = self.recorder.take_result() {
            self.pending = None;
            self.completion = Some(result);
        }
    }

    /// Single-line payload for the ordinary terminal's correlated reply.
    /// Mutations return ACCEPTED; RIDE STATUS reports the retained completion.
    pub fn command(
        &mut self,
        words: &str,
        store: &mut cycling_os::storage::Store<crate::device::storage::Backend<'static>>,
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
            return self.radar_command(operation, argument, bound, store, now, output);
        }
        // Capture appends to the same owned reservation. The ride catalog is
        // intentionally frozen until restart/rescan, so it cannot overwrite it.
        if self.capture_started.is_some() {
            return "CAPTURE_OWNS_STORAGE_RESTART_TO_RESCAN";
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
                    ride_log::VERSION,
                    ride_log::SLOT_SIZE,
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
                    ride_log::transport_checksum(&slot.0)
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
