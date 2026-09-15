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

pub struct Runtime {
    pub clock: Option<fn() -> u64>,
    page: crate::sdk::workout::Page,
    home_cursor: bool,
    scan: crate::sdk::scan::Scan,
    dropped_ant: [Option<u8>; crate::ant::CHANNEL_CAPACITY],
    page_since: u64,
    last_press: u64,
    speed: crate::sdk::workout::Speed,
    live: Sample,
    pressure_base: Option<(u32, u64)>,
    distance_mm: u64,
    metric_at: u64,
    ui_message: &'static str,
    radar: crate::sdk::radar::Radar,
    radar_alert: crate::sdk::radar::Alert,
    stop_armed: Option<u64>,
    next_display: u64,
    display_active: bool,
    menu: crate::sdk::ant_menu::Menu,
    startup_status: &'static str,
    next_reconnect: [u64; crate::ant::CHANNEL_CAPACITY],
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
            page: crate::sdk::workout::Page::Boot,
            home_cursor: false,
            scan: crate::sdk::scan::Scan::default(),
            dropped_ant: [None; crate::ant::CHANNEL_CAPACITY],
            page_since: 0,
            last_press: 0,
            speed: crate::sdk::workout::Speed::default(),
            live: Sample::default(),
            pressure_base: None,
            distance_mm: 0,
            metric_at: 0,
            ui_message: "READY",
            radar: crate::sdk::radar::Radar::new(),
            radar_alert: crate::sdk::radar::Alert::default(),
            stop_armed: None,
            next_display: 0,
            display_active: true,
            menu: crate::sdk::ant_menu::Menu::new(),
            startup_status: "unknown",
            next_reconnect: [0; crate::ant::CHANNEL_CAPACITY],
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

    fn workout_input(
        &mut self,
        input: crate::capabilities::Input,
        now: u64,
        ant: &mut impl crate::capabilities::Ant,
    ) {
        use crate::capabilities::{Button, Input};
        use crate::sdk::workout::Page;
        if self.page == Page::Scan {
            if self.scan.active() {
                return;
            }
            if matches!(
                input,
                Input::Button {
                    button: Button::TopLeft,
                    code: 1
                }
            ) {
                self.page = Page::Sensors;
                self.next_display = 0;
                return;
            }
            self.menu
                .refresh(ant.discoveries(), ant.channels(now), ant.scanning(), now);
            if let Some(action) = self.menu.input(input, now) {
                use crate::sdk::ant_menu::Action;
                let result = match action {
                    Action::Scan => {
                        self.scan.start(ant, now, &self.dropped_ant);
                        self.menu.set_message(self.scan.message());
                        self.next_display = 0;
                        return;
                    }
                    Action::Connect(peer) => {
                        let result =
                            ant.request(crate::capabilities::AntOperation::Connect(peer), now);
                        if result == "ACCEPTED" {
                            for kind in &mut self.dropped_ant {
                                if *kind == Some(peer.device_type) {
                                    *kind = None;
                                }
                            }
                        }
                        result
                    }
                    Action::Disconnect(kind) => {
                        let result =
                            ant.request(crate::capabilities::AntOperation::Disconnect(kind), now);
                        if result == "ACCEPTED" && !self.dropped_ant.contains(&Some(kind)) {
                            // Only selected channels can be dropped; retire entries whose slot
                            // has since been reused by another device type.
                            let channels = ant.channels(now);
                            for dropped in &mut self.dropped_ant {
                                if dropped.is_some_and(|kind| {
                                    !channels
                                        .iter()
                                        .flatten()
                                        .any(|s| s.selected.is_some_and(|p| p.device_type == kind))
                                }) {
                                    *dropped = None;
                                }
                            }
                            if let Some(slot) =
                                self.dropped_ant.iter_mut().find(|slot| slot.is_none())
                            {
                                *slot = Some(kind);
                            }
                        }
                        result
                    }
                    _ => {
                        self.page = Page::Sensors;
                        "OK"
                    }
                };
                self.menu.set_message(if result == "ACCEPTED" {
                    b"REQUEST SENT"
                } else {
                    b"WAIT THEN RETRY"
                });
            }
            self.next_display = 0;
            return;
        }
        let Input::Button { button, code: 1 } = input else {
            return;
        };
        if now.saturating_sub(self.last_press) < 350 {
            return;
        }
        self.last_press = now;
        self.next_display = 0;
        match self.page {
            Page::Boot => {}
            Page::Home => match button {
                Button::BottomLeft => self.home_cursor = !self.home_cursor,
                Button::BottomRight => {
                    self.page = if self.home_cursor {
                        Page::Sensors
                    } else {
                        Page::Preflight
                    };
                    self.page_since = now;
                }
                _ => {}
            },
            Page::Sensors => match button {
                Button::TopLeft | Button::BottomLeft => self.page = Page::Home,
                Button::BottomRight => {
                    self.page = Page::Scan;
                }
                _ => {}
            },
            Page::Preflight => {}
            Page::Ride => {
                if self.pending.is_some() {
                    return;
                }
                let action = match button {
                    Button::BottomLeft => match self.recorder.status() {
                        ride_log::Status::Recording => Some(crate::sdk::ride::Action::Pause),
                        ride_log::Status::Paused => Some(crate::sdk::ride::Action::Resume),
                        ride_log::Status::Ready
                        | ride_log::Status::Saved
                        | ride_log::Status::Recovered => Some(crate::sdk::ride::Action::Start),
                        _ => None,
                    },
                    Button::BottomRight
                        if matches!(
                            self.recorder.status(),
                            ride_log::Status::Recording | ride_log::Status::Paused
                        ) =>
                    {
                        if self
                            .stop_armed
                            .is_some_and(|at| now.saturating_sub(at) <= 5000)
                        {
                            Some(crate::sdk::ride::Action::Finish)
                        } else {
                            self.stop_armed = Some(now);
                            self.ui_message = "PRESS STOP AGAIN TO SAVE";
                            None
                        }
                    }
                    Button::TopLeft if !self.recording() => {
                        self.page = Page::Home;
                        None
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    self.completion = None;
                    let token = self.next_token;
                    if self.recorder.request(action, Source::Live, now, token) {
                        self.pending = Some(token);
                        self.next_token = self.next_token.wrapping_add(1).max(1);
                        self.ui_message = "READY";
                        self.stop_armed = None;
                    } else {
                        self.ui_message = "STORAGE NOT READY";
                    }
                }
            }
            Page::Scan => {}
        }
    }

    fn workout_display<
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
        use crate::sdk::workout::{Page, View};
        if self.page == Page::Boot && now >= 6500 {
            self.page = Page::Home;
        }
        if self.page == Page::Preflight && now.saturating_sub(self.page_since) >= 2500 {
            self.page = Page::Ride;
        }
        let utc = self.live.utc_ms.map(|v| v / 1000).or_else(|| {
            position.snapshot(now).and_then(|p| p.gps.utc).map(|v| {
                u64::from((v[0] - b'0') * 10 + v[1] - b'0') * 3600
                    + u64::from((v[2] - b'0') * 10 + v[3] - b'0') * 60
            })
        });
        let mut view = View::new(self.page, self.home_cursor, utc);
        view.now_ms = now;
        view.sample = self.live;
        view.active_secs = self.recorder.active_ms(now) / 1000;
        view.state = self.recorder.status();
        view.radar = self.radar.snapshot(now);
        view.stop_confirm = self
            .stop_armed
            .is_some_and(|at| now.saturating_sub(at) <= 5000);
        let gps = position.snapshot(now);
        view.gps_fix = gps.is_some_and(|p| p.gps.state == FixState::Fresh);
        let gps_status = gps.map_or("OFF", |p| {
            if p.gps.state == FixState::Fresh {
                "FIX OK"
            } else {
                "SEARCHING"
            }
        });
        match self.page {
            Page::Boot => {
                let _ = match (now / 1000) % 3 {
                    0 => write!(view.lines[0], "> COMPANION {}", self.startup_status),
                    1 => write!(
                        view.lines[0],
                        "> RADIO {:?} / GPS {}",
                        ant.availability(),
                        gps_status
                    ),
                    _ => write!(view.lines[0], "> STORAGE {}", self.recorder.status().name()),
                };
            }
            Page::Sensors | Page::Preflight => {
                let _ = view.lines[0].push_str(if self.page == Page::Preflight {
                    "> TRAIN / CONNECTION CHECK"
                } else {
                    "> CONNECTED SENSORS"
                });
                for (i, kind) in [123, 120, 11, 40].into_iter().enumerate() {
                    let channel = ant.channel(kind, now).or_else(|| {
                        if kind == 123 {
                            ant.channel(121, now)
                        } else {
                            None
                        }
                    });
                    let label = ["SPEED", "HEART", "POWER", "RADAR"][i];
                    if let Some(channel) = channel {
                        let id = channel.selected.map_or(0, |p| p.device_number);
                        let _ = write!(
                            view.lines[i + 2],
                            "{} {} {}",
                            label,
                            id,
                            if channel.link == crate::ant::LinkState::Connected && !channel.stale {
                                "OK"
                            } else {
                                "WAIT"
                            }
                        );
                    } else {
                        let _ = write!(view.lines[i + 2], "{} --", label);
                    }
                }
                let _ = write!(view.lines[6], "GPS {}", gps_status);
                let _ = write!(view.lines[7], "STORAGE {}", self.recorder.status().name());
                let _ = write!(view.lines[8], "RADIO {:?}", ant.availability());
            }
            Page::Ride => {
                let _ = write!(view.lines[0], "> {}", self.recorder.status().name());
                if let Some(v) = self.live.speed_mm_s {
                    let k = v * 36 / 1000;
                    let _ = write!(view.lines[1], "{}.{} KM/H", k / 10, k % 10);
                } else {
                    let _ = view.lines[1].push_str("-- KM/H");
                }
                if let Some(v) = self.live.heart_bpm {
                    let _ = write!(view.lines[2], "{} BPM", v);
                } else {
                    let _ = view.lines[2].push_str("-- BPM");
                }
                if let Some(v) = self.live.power_watts {
                    let _ = write!(view.lines[3], "{} W", v);
                } else {
                    let _ = view.lines[3].push_str("-- W");
                }
                let seconds = self.recorder.active_ms(now) / 1000;
                let _ = write!(
                    view.lines[4],
                    "TIME {:02}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                );
                if let Some(v) = self.live.gradient_tenths {
                    let _ = write!(
                        view.lines[5],
                        "GRADE {}{}.{:01}%",
                        if v < 0 { "-" } else { "+" },
                        v.unsigned_abs() / 10,
                        v.unsigned_abs() % 10
                    );
                } else {
                    let _ = view.lines[5].push_str("GRADE --");
                }
                let _ = write!(
                    view.lines[6],
                    "POWER Z{}  HR Z{}",
                    self.live
                        .power_watts
                        .map_or(0, crate::sdk::workout::power_zone),
                    self.live
                        .heart_bpm
                        .map_or(0, crate::sdk::workout::heart_zone)
                );
                let _ = write!(
                    view.lines[7],
                    "GPS {} / SAVED {}",
                    gps_status,
                    self.recorder.written_samples()
                );
                if self
                    .stop_armed
                    .is_some_and(|at| now.saturating_sub(at) <= 5000)
                {
                    let _ = view.lines[8].push_str("PRESS STOP AGAIN TO SAVE");
                } else if let Some(result) = self.completion {
                    let _ = view.lines[8].push_str(if result.ok { "OK" } else { "WRITE FAILED" });
                } else if self.pending.is_some() {
                    let _ = view.lines[8].push_str("SAVING");
                } else {
                    let _ = view.lines[8].push_str(match self.recorder.status() {
                        ride_log::Status::Recording => "RECORDING / 1 SECOND",
                        ride_log::Status::Paused => "PAUSED / LEFT TO RESUME",
                        ride_log::Status::Saved => "SAVED / TOP LEFT HOME",
                        _ => self.ui_message,
                    });
                }
                system.activity(now);
            }
            _ => {}
        }
        if self.page == Page::Scan {
            self.menu
                .refresh(ant.discoveries(), ant.channels(now), ant.scanning(), now);
        }
        view.prepare();
        system.draw_scaled(240, 320, |x, y| {
            if self.page == Page::Scan && y >= 25 {
                self.menu.pixel(x, y)
            } else {
                view.pixel(x, y)
            }
        });
    }

    pub fn alert(&mut self, sound: &mut impl crate::sound::Sound, now: u64) {
        if self.page != crate::sdk::workout::Page::Ride {
            return;
        }
        if self.radar_alert.update(self.radar.snapshot(now), now)
            && let Some(pattern) = sound
                .patterns()
                .iter()
                .filter(|p| p.nominal_ms <= 300)
                .max_by_key(|p| p.nominal_ms)
        {
            let _ = sound.play(pattern.id, now);
        }
    }

    pub fn select_profile(&mut self, profile: Profile, previous_connection: u32) {
        self.sensors.select_profile(profile, previous_connection);
    }

    pub fn recording(&self) -> bool {
        // Protect accepted writes and active/paused rides from host-triggered restart.
        self.pending.is_some()
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

    pub fn present<
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
        if !self.display_active {
            return;
        }
        if now < self.next_display {
            return;
        }
        self.next_display = now.saturating_add(
            if self.page == crate::sdk::workout::Page::Boot
                || self.page == crate::sdk::workout::Page::Ride
            {
                500
            } else {
                1000
            },
        );
        self.workout_display(system, now, ant, position);
    }

    pub fn set_startup_status(&mut self, status: &'static str) {
        if self.startup_status != status {
            self.startup_status = status;
            self.menu.set_message(match status {
                "charging" | "wake_pending" | "waiting" => b"STARTING SENSORS",
                "probing" | "pending" => b"WAITING FOR COMPANION",
                "unavailable" | "uncertain" | "timed_out" => b"STARTUP FAILED",
                _ => b"SCAN THEN PICK SENSOR",
            });
            self.next_display = 0;
        }
    }

    pub fn suspend_display(&mut self) {
        self.display_active = false;
    }

    pub fn input_active(&self) -> bool {
        self.display_active
    }

    pub fn input(
        &mut self,
        input: crate::capabilities::Input,
        now: u64,
        ant: &mut impl crate::capabilities::Ant,
    ) {
        if self.display_active {
            self.workout_input(input, now, ant);
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
                let heart = self.heart.filter(|(_, at)| now.saturating_sub(*at) < 3000);
                let power = self.power.filter(|(_, at)| now.saturating_sub(*at) < 3000);
                write!(output, "heart={:?} power={:?}", heart, power)
            }
            _ => return "INVALID",
        };
        if result.is_ok() { "OK" } else { "OUTPUT" }
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
        let was_scanning = self.scan.active();
        self.scan.tick(ant, now);
        if was_scanning || self.scan.active() {
            self.menu.set_message(self.scan.message());
        }
        let channels = ant.channels(now);
        for (index, channel) in channels.iter().enumerate() {
            let Some(channel) = channel else { continue };
            let Some(peer) = channel.selected else {
                continue;
            };
            if self.display_active
                && !self.scan.active()
                && !self.dropped_ant.contains(&Some(peer.device_type))
                && channel.link == crate::ant::LinkState::Disconnected
                && now >= self.next_reconnect[index]
                && ant.request(crate::capabilities::AntOperation::Connect(peer), now) == "ACCEPTED"
            {
                self.next_reconnect[index] = now.saturating_add(5000);
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
            if matches!(packet.identity.device_type, 123 | 121) {
                self.speed.receive(packet);
                continue;
            }
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
        let speed_mm_s = [123, 121]
            .into_iter()
            .any(|kind| {
                ant.channel(kind, now)
                    .is_some_and(|s| s.link == crate::ant::LinkState::Connected && !s.stale)
            })
            .then(|| self.speed.value(now))
            .flatten();
        if now >= self.metric_at.saturating_add(1000) {
            let elapsed = now.saturating_sub(self.metric_at).min(3000);
            self.metric_at = now;
            self.distance_mm = self
                .distance_mm
                .saturating_add(u64::from(speed_mm_s.unwrap_or(0)) * elapsed / 1000);
            if let Observation::Fresh { value, .. } = physical.pressure {
                if let Some((pressure, distance)) = self.pressure_base {
                    let travelled = self.distance_mm.saturating_sub(distance);
                    if travelled >= 30_000 {
                        // Local barometric derivative: dh = -8434 * dp / p, over >=30 m.
                        let grade = (i64::from(pressure) - i64::from(value.pressure_centi_pa))
                            * 8_434_000_000i64
                            / i64::from(pressure)
                            / travelled as i64;
                        self.live.gradient_tenths = (grade.abs() <= 400).then_some(grade as i16);
                        self.pressure_base = Some((value.pressure_centi_pa, self.distance_mm));
                    }
                } else {
                    self.pressure_base = Some((value.pressure_centi_pa, self.distance_mm));
                }
            } else {
                self.live.gradient_tenths = None;
                self.pressure_base = None;
            }
            if speed_mm_s.is_none_or(|speed| speed < 1000) {
                self.live.gradient_tenths = None;
                self.pressure_base = None;
            }
        }
        let sample = Sample {
            active_ms: 0,
            utc_ms,
            location_e7,
            demo_speed_mm_s: None,
            heart_bpm: self
                .heart
                .filter(|(_, at)| now.saturating_sub(*at) <= 3000)
                .and_then(|(heart, _)| heart.bpm)
                .map(u16::from)
                .or(heart_bpm),
            cadence_tenths,
            battery_percent,
            speed_mm_s,
            power_watts: self
                .power
                .filter(|(p, at)| now.saturating_sub(*at) <= 3000 && p.watts != u16::MAX)
                .map(|(p, _)| p.watts),
            gradient_tenths: self.live.gradient_tenths,
        };
        self.live = sample;
        let clock = self.clock;
        self.recorder
            .service(store, now, sample, &|| clock.map_or(now, |clock| clock()));
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
mod tests {
    use super::*;

    #[test]
    fn workout_scan_closes_existing_channels_before_scanning() {
        use crate::capabilities::{Ant, AntOperation, Availability, Button, Input};
        struct Radio {
            channels: crate::ant::Channels,
            requests: std::vec::Vec<AntOperation>,
        }
        impl Ant for Radio {
            fn availability(&self) -> Availability {
                Availability::Ready
            }
            fn scanning(&self) -> bool {
                self.channels.scanning()
            }
            fn discoveries(&self) -> [Option<crate::ant::Discovery>; 8] {
                *self.channels.discoveries()
            }
            fn channels(
                &self,
                now: u64,
            ) -> [Option<crate::ant::Snapshot>; crate::ant::CHANNEL_CAPACITY] {
                self.channels.snapshots(now)
            }
            fn take_packet(&mut self) -> Option<crate::ant::Packet> {
                None
            }
            fn request(&mut self, op: AntOperation, now: u64) -> &'static str {
                self.requests.push(op);
                match self.requests.last().unwrap() {
                    AntOperation::Scan(ms) => {
                        if self.channels.begin_scan(now, *ms).is_ok() {
                            "ACCEPTED"
                        } else {
                            "STATE"
                        }
                    }
                    AntOperation::Disconnect(kind) => {
                        let peer = self.channels.channel(*kind, now).unwrap().selected.unwrap();
                        self.channels.disconnect(*kind, now).unwrap();
                        self.channels
                            .receive(crate::ant::Event::Disconnected(peer), now);
                        "ACCEPTED"
                    }
                    AntOperation::Connect(peer) => {
                        self.channels.connect(*peer, now).unwrap();
                        self.channels
                            .receive(crate::ant::Event::Connected(*peer), now);
                        "ACCEPTED"
                    }
                    _ => "ACCEPTED",
                }
            }
        }
        let peer = crate::ant::Identity {
            device_type: 120,
            device_number: 1,
            transmission_type: 1,
        };
        let mut radio = Radio {
            channels: crate::ant::Channels::new(),
            requests: std::vec::Vec::new(),
        };
        radio.channels.connect(peer, 0).unwrap();
        radio
            .channels
            .receive(crate::ant::Event::Connected(peer), 1);
        let mut runtime = Runtime::new(Profile::HeartRate);
        runtime.page = crate::sdk::workout::Page::Scan;
        runtime.workout_input(
            Input::Button {
                button: Button::BottomRight,
                code: 1,
            },
            1000,
            &mut radio,
        );
        assert!(matches!(
            radio.requests.first(),
            Some(AntOperation::Disconnect(120))
        ));
        runtime.scan.tick(&mut radio, 1100);
        assert!(matches!(
            radio.requests.last(),
            Some(AntOperation::Scan(10000))
        ));
        radio.channels.tick(11200);
        runtime.scan.tick(&mut radio, 11200);
        runtime.scan.tick(&mut radio, 11300);
        runtime.scan.tick(&mut radio, 11400);
        assert!(!runtime.scan.active());
        assert_eq!(
            radio.channels.channel(120, 11400).unwrap().link,
            crate::ant::LinkState::Connected
        );

        // Explicit DROP must survive later foreground scans. Select the connected
        // channel row (there are no discovery rows in this radio fixture).
        runtime.workout_input(
            Input::Button {
                button: Button::BottomLeft,
                code: 1,
            },
            12000,
            &mut radio,
        );
        runtime.workout_input(
            Input::Button {
                button: Button::BottomRight,
                code: 1,
            },
            12500,
            &mut radio,
        );
        assert!(runtime.dropped_ant.contains(&Some(120)));
        assert_eq!(
            radio.channels.channel(120, 12500).unwrap().link,
            crate::ant::LinkState::Disconnected
        );
        radio.requests.clear();
        runtime.scan.start(&mut radio, 13000, &runtime.dropped_ant);
        radio.channels.tick(23200);
        runtime.scan.tick(&mut radio, 23200);
        runtime.scan.tick(&mut radio, 23300);
        assert!(!runtime.scan.active());
        assert!(
            !radio
                .requests
                .iter()
                .any(|request| matches!(request, AntOperation::Connect(_)))
        );
        assert_eq!(
            radio.channels.channel(120, 23300).unwrap().link,
            crate::ant::LinkState::Disconnected
        );

        // An explicit choice from discoveries re-enables this device type.
        radio.channels.begin_scan(23900, 1000).unwrap();
        radio.channels.receive(
            crate::ant::Event::Discovery {
                identity: peer,
                rssi: -40,
            },
            24000,
        );
        radio.channels.receive(crate::ant::Event::ScanEnded, 24100);
        runtime.menu = crate::sdk::ant_menu::Menu::new();

        runtime.workout_input(
            Input::Button {
                button: Button::BottomLeft,
                code: 1,
            },
            24500,
            &mut radio,
        );
        runtime.workout_input(
            Input::Button {
                button: Button::BottomRight,
                code: 1,
            },
            25000,
            &mut radio,
        );
        assert!(!runtime.dropped_ant.contains(&Some(120)));
        assert_eq!(
            radio.channels.channel(120, 25000).unwrap().link,
            crate::ant::LinkState::Connected
        );
    }
}
