//! Composition of core snapshots and the portable cycling SDK.
//! Main calls tick independently of terminal requests. Flash work remains
//! synchronous and bounded to one recorder step, with no lock held across it.
use crate::{
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
use core::fmt::Write;

/// Reserve two local samples per second and, when selected, three ANT slots per
/// second. This is a bounded planning allowance, not a guarantee at arbitrary RF rates.
fn foundation_reservation(seconds: u32, ant_selected: bool) -> Option<usize> {
    if !(300..=600).contains(&seconds) {
        return None;
    }
    Some(seconds as usize * if ant_selected { 5 } else { 2 } + 8)
}

fn foundation_expired(
    now: u64,
    status: crate::sdk::ant_capture::Status,
    seconds: Option<u32>,
    deadline: &mut Option<u64>,
) -> bool {
    use crate::sdk::ant_capture::Status;
    if !matches!(status, Status::Ready | Status::Recording) {
        return false;
    }
    let Some(seconds) = seconds else {
        return false;
    };
    now >= *deadline.get_or_insert_with(|| now.saturating_add(u64::from(seconds) * 1000))
}

pub struct Runtime {
    pub clock: Option<fn() -> u64>,
    radar: crate::sdk::radar::Radar,
    capture: crate::sdk::ant_capture::Capturer,
    capture_started: Option<u64>,
    capture_seconds: Option<u32>,
    capture_deadline: Option<u64>,
    next_display: u64,
    next_position: u64,
    next_reconnect: [u64; 3],
    capture_link: [Option<(u8, crate::ant::LinkState, u32)>; 3],
    ant_epochs: [Option<(u32, u32)>; 3],
    heart: Option<(crate::sdk::ant_sensors::HeartRate, u64)>,
    power: Option<(crate::sdk::ant_sensors::Power, u64)>,
    recorder: Recorder,
    sensors: Client,
    next_token: u32,
    pending: Option<u32>,
    completion: Option<ResultEvent>,
}

impl Runtime {
    pub fn new(profile: Profile) -> Self {
        Self {
            clock: None,
            radar: crate::sdk::radar::Radar::new(),
            capture: crate::sdk::ant_capture::Capturer::new(),
            capture_started: None,
            capture_seconds: None,
            capture_deadline: None,
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

    pub fn select_profile(&mut self, profile: Profile, previous_connection: u32) {
        self.sensors.select_profile(profile, previous_connection);
    }

    pub fn recording(&self) -> bool {
        // INFO protects all accepted work, including acquisition preflight and
        // final flushes, from host-triggered restart. Durable sample counters
        // remain separate in LOG STATUS.
        self.pending.is_some()
            || matches!(
                self.capture.snapshot().status,
                crate::sdk::ant_capture::Status::Scanning
                    | crate::sdk::ant_capture::Status::Ready
                    | crate::sdk::ant_capture::Status::Recording
                    | crate::sdk::ant_capture::Status::Stopping
            )
            || matches!(
                self.recorder.status(),
                ride_log::Status::Recording
                    | ride_log::Status::Paused
                    | ride_log::Status::Formatting
                    | ride_log::Status::Clearing
            )
    }

    pub fn radar(&self, now: u64) -> crate::sdk::radar::Snapshot {
        self.radar.snapshot(now)
    }

    pub fn test_display<
        D: crate::capabilities::Display,
        I: crate::capabilities::InputSource,
        P: crate::capabilities::Power,
        B: crate::storage::OwnedFlash,
    >(
        &mut self,
        system: &mut crate::shell::Shell<D, I, P, B>,
        now: u64,
        ant: &impl crate::capabilities::Ant,
        position: &impl crate::capabilities::Positioning,
    ) {
        if system.foreground == crate::shell::Screen::Blank {
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
        let screen = crate::sdk::radar_screen::Screen {
            sensors: [40, 120, 11].map(|kind| {
                use crate::sdk::radar_screen::SensorState;
                match ant.channel(kind, now) {
                    None => SensorState::Off,
                    Some(s) if s.link == crate::ant::LinkState::Connected && !s.stale => {
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
                || capture.status == crate::sdk::ant_capture::Status::Full,
        };
        system.activity(now);
        let started = self.clock.map_or(now, |clock| clock());
        system.draw_scaled(240, 320, |x, y| {
            crate::sdk::radar_screen::pixel(x, y, screen)
        });
        system.display_max_ms = system.display_max_ms.max(
            self.clock
                .map_or(now, |clock| clock())
                .saturating_sub(started),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn radar_command(
        &mut self,
        operation: Option<&str>,
        argument: Option<&str>,
        bound: Option<&str>,
        store: &mut impl crate::sdk::storage::RideStorage,
        now: u64,
        output: &mut impl Write,
        ant: &impl crate::capabilities::Ant,
    ) -> &'static str {
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
        let channels = ant.channels(now);
        let sensors_ready = channels
            .iter()
            .flatten()
            .any(|s| s.selected.is_some_and(|peer| peer.device_type == 40))
            && !channels
                .iter()
                .flatten()
                .any(|s| s.link != crate::ant::LinkState::Connected || s.stale);
        self.capture_command(argument, bound, store, now, output, sensors_ready, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_command(
        &mut self,
        argument: Option<&str>,
        bound: Option<&str>,
        store: &mut impl crate::sdk::storage::RideStorage,
        now: u64,
        output: &mut impl Write,
        sensors_ready: bool,
        foundation: Option<(u32, usize)>,
    ) -> &'static str {
        use crate::sdk::ant_capture::Status as CaptureStatus;
        match (argument, bound) {
            (Some("START"), None) => {
                if self.capture_started.is_some()
                    || self.pending.is_some()
                    || self.completion.is_some()
                    || !self.recorder.exportable()
                {
                    return "BUSY";
                }
                if !sensors_ready {
                    return "SENSORS_NOT_READY";
                }
                let required =
                    foundation.map_or(crate::sdk::ant_capture::REQUIRED_SLOTS, |(_, slots)| slots);
                if !self.capture.start(required) {
                    return "STATE";
                }
                self.capture_started = Some(now);
                self.capture_seconds = foundation.map(|(seconds, _)| seconds);
                self.capture_deadline = None;
                self.next_position = now;
                "ACCEPTED"
            }
            (Some("STOP"), None) => {
                self.capture.stop();
                "ACCEPTED"
            }
            (Some("STATUS"), None) => {
                let active = matches!(
                    self.capture.snapshot().status,
                    CaptureStatus::Ready | CaptureStatus::Recording
                );
                let remaining = self
                    .capture_deadline
                    .filter(|_| active)
                    .map(|deadline| deadline.saturating_sub(now).div_ceil(1000));
                let _ = write!(
                    output,
                    "{:?} requested_seconds={:?} remaining_seconds={:?}",
                    self.capture.snapshot(),
                    self.capture_seconds,
                    remaining
                );
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

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        store: &mut impl crate::sdk::storage::RideStorage,
        now: u64,
        ant: &mut impl crate::capabilities::Ant,
        ble: &mut impl crate::capabilities::Ble,
        position: &impl crate::capabilities::Positioning,
        network_online: bool,
        input: &impl crate::capabilities::InputObservation,
        physical: crate::companion_sensors::Snapshot,
    ) {
        let channels = ant.channels(now);
        let mut capture_status = self.capture.snapshot().status;
        if foundation_expired(
            now,
            capture_status,
            self.capture_seconds,
            &mut self.capture_deadline,
        ) {
            self.capture.stop();
            capture_status = self.capture.snapshot().status;
        }
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
                    crate::sdk::ant_capture::Status::Ready
                        | crate::sdk::ant_capture::Status::Recording
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
                    crate::sdk::ant_capture::Status::Scanning
                        | crate::sdk::ant_capture::Status::Ready
                        | crate::sdk::ant_capture::Status::Recording
                ) && channel_state.link == crate::ant::LinkState::Disconnected
                    && now >= self.next_reconnect[index]
                {
                    let result = ant.request(crate::capabilities::AntOperation::Connect(peer), now);
                    if result == "ACCEPTED" {
                        self.next_reconnect[index] = now.saturating_add(5000);
                    }
                }
            }
        }
        for (index, kind) in [40, 120, 11].iter().enumerate() {
            let channel = ant.channel(*kind, now);
            if channel.is_none_or(|s| {
                s.link != crate::ant::LinkState::Connected
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
        for _ in 0..crate::ant::PACKET_CAPACITY * crate::ant::CHANNEL_CAPACITY {
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
                        crate::sdk::ant_sensors::HeartRate::decode(packet.data),
                        packet.received_ms,
                    ))
                }
                _ => {
                    if let Some(power) = crate::sdk::ant_sensors::Power::decode(packet.data) {
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
        let clock = crate::network_time::snapshot(now, 0, network_online);
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
                crate::sdk::ant_capture::Status::Ready | crate::sdk::ant_capture::Status::Recording
            ) && now >= self.next_position
            {
                self.next_position = now.saturating_add(1000);
                let observed_ms = position
                    .snapshot(now)
                    .and_then(|s| s.gps.age_ms)
                    .and_then(|age| now.checked_sub(age));
                self.capture
                    .environment(crate::sdk::ant_capture::EnvironmentRecord::sample(
                        now,
                        physical,
                        input.snapshot(now),
                    ));
                self.capture
                    .position(crate::sdk::ant_capture::PositionRecord {
                        now,
                        observed_ms,
                        latitude_e7: location_e7.map(|p| p.0),
                        longitude_e7: location_e7.map(|p| p.1),
                    });
            }
            self.capture.service(store, now);
        } else {
            let clock = self.clock;
            self.recorder
                .service(store, now, sample, &|| clock.map_or(now, |clock| clock()));
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
        store: &mut impl crate::sdk::storage::RideStorage,
        now: u64,
        output: &mut impl Write,
        ant: &impl crate::capabilities::Ant,
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
        if domain == Some("FOUNDATION") {
            if operation != Some("LOG") {
                return "INVALID";
            }
            if argument == Some("START") {
                let Some(seconds) = bound.and_then(|value| value.parse::<u32>().ok()) else {
                    return "INVALID";
                };
                let selected = ant
                    .channels(now)
                    .iter()
                    .flatten()
                    .any(|channel| channel.selected.is_some());
                let Some(required) = foundation_reservation(seconds, selected) else {
                    return "INVALID";
                };
                return self.capture_command(
                    argument,
                    None,
                    store,
                    now,
                    output,
                    true,
                    Some((seconds, required)),
                );
            }
            return self.capture_command(argument, bound, store, now, output, true, None);
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

#[cfg(test)]
mod foundation_tests {
    use super::*;
    use crate::sdk::ant_capture::Status;

    #[test]
    fn duration_reservation_rejects_unbounded_runs_and_accounts_for_selected_ant() {
        assert_eq!(foundation_reservation(300, false), Some(608));
        assert_eq!(foundation_reservation(360, false), Some(728));
        assert_eq!(foundation_reservation(600, false), Some(1208));
        assert_eq!(foundation_reservation(300, true), Some(1508));
        assert_eq!(foundation_reservation(600, true), Some(3008));
        for seconds in [0, 299, 601, u32::MAX] {
            assert_eq!(foundation_reservation(seconds, false), None);
        }
    }

    #[test]
    fn deadline_begins_after_scan_and_expires_without_usb_or_peer() {
        let mut deadline = None;
        assert!(!foundation_expired(
            1_000,
            Status::Scanning,
            Some(300),
            &mut deadline
        ));
        assert_eq!(deadline, None);
        assert!(!foundation_expired(
            9_000,
            Status::Ready,
            Some(300),
            &mut deadline
        ));
        assert_eq!(deadline, Some(309_000));
        assert!(!foundation_expired(
            308_999,
            Status::Recording,
            Some(300),
            &mut deadline
        ));
        assert!(foundation_expired(
            309_000,
            Status::Recording,
            Some(300),
            &mut deadline
        ));
        assert!(!foundation_expired(
            310_000,
            Status::Stopping,
            Some(300),
            &mut deadline
        ));
        let mut radar = None;
        assert!(!foundation_expired(
            1_000_000,
            Status::Recording,
            None,
            &mut radar
        ));
        assert_eq!(radar, None);
    }
}
