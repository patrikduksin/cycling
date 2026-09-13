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
    },
};

pub struct Runtime {
    radar: cycling_os::sdk::radar::Radar,
    capture: cycling_os::sdk::ant_capture::Capturer,
    capture_started: Option<u64>,
    next_display: u64,
    next_position: u64,
    next_reconnect: [u64; 3],
    capture_link: [Option<(u8, cycling_os::ant::LinkState, u32)>; 3],
    ant_epochs: [Option<(u32, u32)>; 3],
    heart: Option<(cycling_os::sdk::ant_sensors::HeartRate, u64)>,
    power: Option<(cycling_os::sdk::ant_sensors::Power, u64)>,
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
            next_position: 0,
            next_reconnect: [0; 3],
            capture_link: [None; 3],
            ant_epochs: [None; 3],
            heart: None,
            power: None,
            recorder: Recorder::default(),
            sensors: Client::new(profile),
            next_token: 1,
            pending: None,
            completion: None,
        }
    }

    pub fn recording(&self) -> bool {
        self.capture.snapshot().recording || self.recorder.status() == ride_log::Status::Recording
    }

    pub fn radar(&self, now: u64) -> cycling_os::sdk::radar::Snapshot {
        self.radar.snapshot(now)
    }

    pub fn test_display<
        D: cycling_os::capabilities::Display,
        I: cycling_os::capabilities::InputSource,
        P: cycling_os::capabilities::Power,
        B: cycling_os::storage::OwnedFlash,
    >(
        &mut self,
        system: &mut cycling_os::shell::Shell<D, I, P, B>,
        now: u64,
        ant: &impl cycling_os::capabilities::Ant,
        position: &impl cycling_os::capabilities::Positioning,
    ) {
        if system.foreground == cycling_os::shell::Screen::Blank {
            return;
        }
        let Some(started) = self.capture_started else {
            return;
        };
        if now < self.next_display {
            return;
        }
        self.next_display = now.saturating_add(1000);
        let channels = ant.channels(now);
        let capture = self.capture.snapshot();
        let screen = cycling_os::sdk::radar_screen::Screen {
            sensors: [40, 120, 11].map(|kind| {
                use cycling_os::sdk::radar_screen::SensorState;
                match ant.channel(kind, now) {
                    None => SensorState::Off,
                    Some(s) if s.link == cycling_os::ant::LinkState::Connected && !s.stale => {
                        SensorState::On
                    }
                    _ => SensorState::Wait,
                }
            }),
            logging: capture.recording
                && capture.packets > 0
                && capture
                    .last_commit_ms
                    .is_some_and(|at| now.saturating_sub(at) < 3000),
            gps_fix: position.snapshot(now).is_some_and(|s| {
                s.gps.state == FixState::Fresh
                    && s.gps.latitude_e7.is_some()
                    && s.gps.longitude_e7.is_some()
            }),
            saved_positions: capture.saved_positions,
            saved_packets: capture.packets,
            dropped: capture
                .dropped
                .saturating_add(capture.dropped_links)
                .saturating_add(capture.dropped_positions)
                .saturating_add(
                    channels
                        .iter()
                        .flatten()
                        .fold(0u32, |sum, s| sum.saturating_add(s.dropped_packets)),
                ),
            elapsed_secs: (now.saturating_sub(started) / 1000).min(u64::from(u32::MAX)) as u32,
            error: capture.error.is_some()
                || capture.status == cycling_os::sdk::ant_capture::Status::Full,
        };
        system.activity(now);
        let started = embassy_time::Instant::now();
        system.draw_scaled(240, 320, |x, y| {
            cycling_os::sdk::radar_screen::pixel(x, y, screen)
        });
        system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
    }

    fn radar_command(
        &mut self,
        operation: Option<&str>,
        argument: Option<&str>,
        bound: Option<&str>,
        store: &mut impl cycling_os::sdk::storage::RideStorage,
        now: u64,
        output: &mut impl Write,
        ant: &impl cycling_os::capabilities::Ant,
    ) -> &'static str {
        use cycling_os::sdk::ant_capture::Status as CaptureStatus;
        if operation.is_none() && argument.is_none() && bound.is_none() {
            let _ = write!(output, "{:?}", self.radar(now));
            return "OK";
        }
        if operation == Some("SENSORS") && argument.is_none() && bound.is_none() {
            let heart = self.heart.filter(|(_, at)| now.saturating_sub(*at) < 3000);
            let power = self.power.filter(|(_, at)| now.saturating_sub(*at) < 3000);
            let _ = write!(output, "heart={:?} power={:?}", heart, power);
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
                let channels = ant.channels(now);
                if !channels
                    .iter()
                    .flatten()
                    .any(|s| s.selected.is_some_and(|peer| peer.device_type == 40))
                    || channels
                        .iter()
                        .flatten()
                        .any(|s| s.link != cycling_os::ant::LinkState::Connected || s.stale)
                {
                    return "SENSORS_NOT_READY";
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
        store: &mut impl cycling_os::sdk::storage::RideStorage,
        now: u64,
        ant: &mut impl cycling_os::capabilities::Ant,
        ble: &mut impl cycling_os::capabilities::Ble,
        position: &impl cycling_os::capabilities::Positioning,
        network: &impl cycling_os::capabilities::Network,
        input: &impl cycling_os::capabilities::InputObservation,
    ) {
        let channels = ant.channels(now);
        let capture_status = self.capture.snapshot().status;
        for (index, channel) in channels.iter().enumerate() {
            let Some(channel_state) = channel else {
                continue;
            };
            let Some(peer) = channel_state.selected else {
                continue;
            };
            if self.capture_started.is_some() {
                if matches!(
                    capture_status,
                    cycling_os::sdk::ant_capture::Status::Ready
                        | cycling_os::sdk::ant_capture::Status::Recording
                ) && self.capture_link[index]
                    != Some((
                        peer.device_type,
                        channel_state.link,
                        channel_state.generation,
                    ))
                {
                    self.capture.observe_link(
                        peer.device_type,
                        channel_state.link,
                        channel_state.generation,
                        now,
                    );
                    self.capture_link[index] = Some((
                        peer.device_type,
                        channel_state.link,
                        channel_state.generation,
                    ));
                }
                if matches!(
                    capture_status,
                    cycling_os::sdk::ant_capture::Status::Scanning
                        | cycling_os::sdk::ant_capture::Status::Ready
                        | cycling_os::sdk::ant_capture::Status::Recording
                ) && channel_state.link == cycling_os::ant::LinkState::Disconnected
                    && now >= self.next_reconnect[index]
                {
                    let result =
                        ant.request(cycling_os::capabilities::AntOperation::Connect(peer), now);
                    if result == "ACCEPTED" {
                        self.next_reconnect[index] = now.saturating_add(5000);
                    }
                }
            }
        }
        for (index, kind) in [40, 120, 11].iter().enumerate() {
            let channel = ant.channel(*kind, now);
            if channel.is_none_or(|s| {
                s.link != cycling_os::ant::LinkState::Connected
                    || self.ant_epochs[index]
                        .is_some_and(|(generation, _)| generation != s.generation)
            }) {
                match index {
                    0 => self.radar.reset(),
                    1 => self.heart = None,
                    _ => self.power = None,
                }
                self.ant_epochs[index] = None;
            }
        }
        for _ in 0..cycling_os::ant::PACKET_CAPACITY * cycling_os::ant::CHANNEL_CAPACITY {
            let Some(packet) = ant.take_packet() else {
                break;
            };
            self.capture.packet(packet);
            let Some(index) = [40, 120, 11]
                .iter()
                .position(|kind| *kind == packet.identity.device_type)
            else {
                continue;
            };
            let epoch = (packet.generation, packet.loss_count);
            if self.ant_epochs[index] != Some(epoch) {
                match index {
                    0 => self.radar.reset(),
                    1 => self.heart = None,
                    _ => self.power = None,
                }
                self.ant_epochs[index] = Some(epoch);
            }
            match index {
                0 => {
                    self.radar.receive(packet.data, packet.received_ms);
                }
                1 => {
                    self.heart = Some((
                        cycling_os::sdk::ant_sensors::HeartRate::decode(packet.data),
                        packet.received_ms,
                    ))
                }
                _ => {
                    if let Some(power) = cycling_os::sdk::ant_sensors::Power::decode(packet.data) {
                        self.power = Some((power, packet.received_ms));
                    }
                }
            }
        }
        let transport = ble.snapshot();
        self.sensors.update(transport);
        // Core queue has two packets. Never drain an unbounded producer here.
        for _ in 0..2 {
            let Some(packet) = ble.take_packet() else {
                break;
            };
            self.sensors.packet(packet);
        }
        let sensors = self.sensors.snapshot(now);
        let (heart_bpm, cadence_tenths) = ble_sensor::ride_fields(sensors, self.recorder.source());
        let location_e7 = position.snapshot(now).and_then(|snapshot| {
            (snapshot.gps.state == FixState::Fresh)
                .then(|| Some((snapshot.gps.latitude_e7?, snapshot.gps.longitude_e7?)))
                .flatten()
        });
        let clock = cycling_os::network_time::snapshot(now, 0, network.online());
        let utc_ms = clock.unix_seconds.and_then(|seconds| {
            seconds
                .checked_mul(1_000)?
                .checked_add(u64::from(clock.millis))
        });
        // Keep existing last-observed battery behavior. Core status still exposes
        // its age; these ride format bytes have no battery-age field.
        let battery_percent = input
            .snapshot(now)
            .and_then(|snapshot| match snapshot.battery {
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
            if matches!(
                capture_status,
                cycling_os::sdk::ant_capture::Status::Ready
                    | cycling_os::sdk::ant_capture::Status::Recording
            ) && now >= self.next_position
            {
                self.next_position = now.saturating_add(1000);
                let observed_ms = position
                    .snapshot(now)
                    .and_then(|s| s.gps.age_ms)
                    .and_then(|age| now.checked_sub(age));
                self.capture
                    .position(cycling_os::sdk::ant_capture::PositionRecord {
                        now,
                        observed_ms,
                        latitude_e7: location_e7.map(|p| p.0),
                        longitude_e7: location_e7.map(|p| p.1),
                    });
            }
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
        store: &mut impl cycling_os::sdk::storage::RideStorage,
        now: u64,
        output: &mut impl Write,
        ant: &impl cycling_os::capabilities::Ant,
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
            return self.radar_command(operation, argument, bound, store, now, output, ant);
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
