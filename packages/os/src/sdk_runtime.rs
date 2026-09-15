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

fn sampled_reservation(seconds: u32) -> Option<usize> {
    (300..=600).contains(&seconds).then(|| {
        // One environment, one position and at most one three-peer packet batch
        // every two seconds; at most three link snapshots every ten seconds.
        seconds.div_ceil(2) as usize * 3 + seconds.div_ceil(10) as usize * 3 + 8
    })
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
    page: crate::sdk::workout::Page,
    workout_ui: bool,
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
    capture: crate::sdk::ant_capture::Capturer,
    capture_started: Option<u64>,
    capture_seconds: Option<u32>,
    capture_deadline: Option<u64>,
    recording_since: Option<u64>,
    recording_ended: Option<u64>,
    stop_armed: Option<u64>,
    next_display: u64,
    next_position: u64,
    next_capture_link: u64,
    sampled_packets: [Option<crate::ant::Packet>; 3],
    foundation_screen: bool,
    menu: crate::sdk::ant_menu::Menu,
    menu_open: bool,
    gps_fix_ms: Option<u64>,
    startup_status: &'static str,
    preview_seconds: Option<u32>,
    next_reconnect: [u64; crate::ant::CHANNEL_CAPACITY],
    capture_link: [Option<(u8, crate::ant::LinkState, u32)>; crate::ant::CHANNEL_CAPACITY],
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
            workout_ui: true,
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
            capture: crate::sdk::ant_capture::Capturer::new(),
            capture_started: None,
            capture_seconds: None,
            capture_deadline: None,
            recording_since: None,
            recording_ended: None,
            stop_armed: None,
            next_display: 0,
            next_position: 0,
            next_capture_link: 0,
            sampled_packets: [None; 3],
            foundation_screen: true,
            menu: crate::sdk::ant_menu::Menu::new(),
            menu_open: true,
            gps_fix_ms: None,
            startup_status: "unknown",
            preview_seconds: None,
            next_reconnect: [0; crate::ant::CHANNEL_CAPACITY],
            capture_link: [None; crate::ant::CHANNEL_CAPACITY],
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
                    self.menu.workout();
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
        if !self.foundation_screen {
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
        if self.workout_ui && self.capture_started.is_none() {
            self.workout_display(system, now, ant, position);
            return;
        }
        if self.menu_open && self.capture_started.is_none() {
            self.menu
                .refresh(ant.discoveries(), ant.channels(now), ant.scanning(), now);
            let started = self.clock.map_or(now, |clock| clock());
            system.draw_scaled(240, 320, |x, y| self.menu.pixel(x, y));
            system.display_max_ms = system.display_max_ms.max(
                self.clock
                    .map_or(now, |clock| clock())
                    .saturating_sub(started),
            );
            return;
        }
        let channels = ant.channels(now);
        let capture = self.capture.snapshot();
        use crate::sdk::{ant_capture::Status, radar_screen::CaptureState};
        let state = match capture.status {
            Status::Idle if !self.recorder.exportable() => CaptureState::Preparing,
            Status::Idle => CaptureState::Idle,
            Status::Scanning | Status::Ready => CaptureState::Preparing,
            Status::Recording => CaptureState::Recording,
            Status::Stopping => CaptureState::Stopping,
            Status::Stopped => CaptureState::Stopped,
            Status::Full => CaptureState::Full,
            Status::Error => CaptureState::Error,
        };
        let remaining_secs = self.capture_seconds.map(|seconds| {
            if matches!(
                capture.status,
                Status::Stopped | Status::Full | Status::Error
            ) {
                0
            } else {
                self.capture_deadline
                    .map_or(seconds, |at| at.saturating_sub(now).div_ceil(1000) as u32)
            }
        });
        let elapsed_secs = self
            .recording_since
            .map_or(0, |at| {
                self.recording_ended.unwrap_or(now).saturating_sub(at) / 1000
            })
            .min(u64::from(u32::MAX)) as u32;
        let screen = crate::sdk::radar_screen::Screen {
            state,
            remaining_secs,
            stop_confirm: self
                .stop_armed
                .is_some_and(|at| now.saturating_sub(at) <= 5000),
            free_slots: if self.capture_started.is_some() {
                (capture.scanned == ride_log::SECTORS).then_some(capture.remaining_slots as u32)
            } else {
                self.recorder
                    .exportable()
                    .then_some((ride_log::SLOTS - self.recorder.next_slot()) as u32)
            },
            saved_environment: capture.saved_environment,
            power_watts: self
                .power
                .filter(|(power, at)| now.saturating_sub(*at) <= 3000 && power.watts != u16::MAX)
                .map(|(power, _)| power.watts),
            radar_age_secs: ant
                .channel(40, now)
                .and_then(|s| s.age_ms)
                .map(|age| (age / 1000).min(u64::from(u32::MAX)) as u32),
            power_age_secs: self
                .power
                .map(|(_, at)| (now.saturating_sub(at) / 1000).min(u64::from(u32::MAX)) as u32),
            sampled: capture.sampled || (self.capture_started.is_none() && self.foundation_screen),
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
                && capture
                    .last_commit_ms
                    .is_some_and(|at| now.saturating_sub(at) < 5000),
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
                .saturating_add(capture.dropped_environment)
                .saturating_add(
                    channels
                        .iter()
                        .flatten()
                        .fold(0u32, |sum, s| sum.saturating_add(s.dropped_packets)),
                ),
            elapsed_secs,
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
        self.foundation_screen = false;
        self.menu_open = false;
    }

    pub fn input_active(&self) -> bool {
        self.foundation_screen
    }

    pub fn input(
        &mut self,
        input: crate::capabilities::Input,
        now: u64,
        ant: &mut impl crate::capabilities::Ant,
        _store: &mut impl crate::sdk::storage::RideStorage,
    ) {
        use crate::sdk::ant_menu::Action;
        if !self.foundation_screen {
            return;
        }
        if self.workout_ui && self.capture_started.is_none() {
            self.workout_input(input, now, ant);
            return;
        }
        if self.capture_started.is_some() {
            if matches!(
                self.capture.snapshot().status,
                crate::sdk::ant_capture::Status::Scanning
                    | crate::sdk::ant_capture::Status::Ready
                    | crate::sdk::ant_capture::Status::Recording
            ) {
                self.next_display = now;
                match input {
                    crate::capabilities::Input::Button {
                        button: crate::capabilities::Button::BottomRight,
                        code: 1,
                    } => match self.stop_armed {
                        Some(at) if (350..=5000).contains(&now.saturating_sub(at)) => {
                            self.stop_capture()
                        }
                        Some(at) if now.saturating_sub(at) < 350 => {}
                        _ => self.stop_armed = Some(now),
                    },
                    _ => self.stop_armed = None,
                }
            }
            return;
        }
        if !self.menu_open {
            if matches!(
                input,
                crate::capabilities::Input::Button {
                    button: crate::capabilities::Button::TopLeft,
                    code: 1
                }
            ) {
                self.menu_open = true;
                self.next_display = now;
            }
            return;
        }
        self.menu
            .refresh(ant.discoveries(), ant.channels(now), ant.scanning(), now);
        self.next_display = now;
        let Some(action) = self.menu.input(input, now) else {
            return;
        };
        let result = match action {
            Action::Scan => ant.request(crate::capabilities::AntOperation::Scan(10_000), now),
            Action::Connect(peer) => {
                ant.request(crate::capabilities::AntOperation::Connect(peer), now)
            }
            Action::Disconnect(kind) => {
                ant.request(crate::capabilities::AntOperation::Disconnect(kind), now)
            }
            Action::Ride => {
                self.menu_open = false;
                return;
            }
            Action::Start => {
                let ready = [40, 11].into_iter().all(|kind| {
                    ant.channel(kind, now).is_some_and(|s| {
                        s.selected.is_some()
                            && s.link == crate::ant::LinkState::Connected
                            && !s.stale
                    })
                });
                if !ready {
                    self.menu.set_message(b"WAKE RADAR AND POWER");
                    return;
                }
                if !self
                    .gps_fix_ms
                    .is_some_and(|at| now >= at && now - at <= crate::gps::STALE_MS)
                {
                    self.menu.set_message(b"WAIT FOR GPS OUTSIDE");
                    return;
                }
                let required = 8;
                if !self.recorder.exportable() {
                    self.menu.set_message(b"WAIT STORAGE SCAN");
                    return;
                }
                if ride_log::SLOTS - self.recorder.next_slot() < required {
                    self.menu.set_message(b"NOT ENOUGH STORAGE");
                    return;
                }
                let result = self.start_capture(now, true, Some((0, required, true)));
                if result == "ACCEPTED" {
                    self.menu_open = false;
                }
                result
            }
        };
        self.menu.set_message(match result {
            "ACCEPTED" => b"REQUEST SENT",
            "BUSY" if matches!(self.startup_status, "charging" | "wake_pending" | "waiting") => {
                b"STARTING SENSORS"
            }
            "BUSY" if matches!(self.startup_status, "probing" | "pending") => {
                b"WAITING FOR COMPANION"
            }
            "BUSY"
                if matches!(
                    self.startup_status,
                    "unavailable" | "uncertain" | "timed_out"
                ) =>
            {
                b"STARTUP FAILED"
            }
            "BUSY" => b"RADIO BUSY WAIT THEN RETRY",
            "OK" => b"OK",
            _ => b"REQUEST NOT ACCEPTED",
        });
        self.next_display = now;
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

    fn stop_capture(&mut self) {
        for packet in &mut self.sampled_packets {
            if packet.take().is_some() {
                self.capture.sampled_out_packet();
            }
        }
        self.capture.stop();
        self.stop_armed = None;
    }

    fn start_capture(
        &mut self,
        now: u64,
        sensors_ready: bool,
        foundation: Option<(u32, usize, bool)>,
    ) -> &'static str {
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
        let required = foundation
            .map_or(crate::sdk::ant_capture::REQUIRED_SLOTS, |(_, slots, _)| {
                slots
            });
        let sampled = foundation.is_some_and(|(_, _, sampled)| sampled);
        if !(if sampled {
            self.capture.start_sampled(required)
        } else {
            self.capture.start(required)
        }) {
            return "STATE";
        }
        self.capture_started = Some(now);
        self.capture_seconds = foundation
            .map(|(seconds, _, _)| seconds)
            .filter(|seconds| *seconds != 0);
        self.recording_since = None;
        self.recording_ended = None;
        self.stop_armed = None;
        self.capture_deadline = None;
        self.next_position = now;
        self.next_capture_link = now;
        self.sampled_packets = [None; 3];
        self.foundation_screen = true;
        "ACCEPTED"
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
        foundation: Option<(u32, usize, bool)>,
    ) -> &'static str {
        use crate::sdk::ant_capture::Status as CaptureStatus;
        match (argument, bound) {
            (Some("START"), None) => self.start_capture(now, sensors_ready, foundation),
            (Some("STOP"), None) => {
                self.stop_capture();
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
                    "{:?} requested_seconds={:?} remaining_seconds={:?} sample_interval_ms={} link_interval_ms={}",
                    self.capture.snapshot(),
                    self.capture_seconds,
                    remaining,
                    if self.capture.snapshot().sampled {
                        2000
                    } else {
                        1000
                    },
                    if self.capture.snapshot().sampled {
                        10000
                    } else {
                        0
                    }
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
        self.gps_fix_ms = position.snapshot(now).and_then(|s| {
            if s.gps.state == FixState::Fresh
                && s.gps.latitude_e7.is_some()
                && s.gps.longitude_e7.is_some()
            {
                now.checked_sub(s.gps.age_ms?)
            } else {
                None
            }
        });
        let was_scanning = self.scan.active();
        self.scan.tick(ant, now);
        if was_scanning || self.scan.active() {
            self.menu.set_message(self.scan.message());
        }
        let channels = ant.channels(now);
        let mut capture_status = self.capture.snapshot().status;
        let sampled = self.capture.snapshot().sampled;
        if matches!(
            capture_status,
            crate::sdk::ant_capture::Status::Ready | crate::sdk::ant_capture::Status::Recording
        ) {
            self.recording_since.get_or_insert(now);
        } else if self.recording_since.is_some()
            && matches!(
                capture_status,
                crate::sdk::ant_capture::Status::Stopped
                    | crate::sdk::ant_capture::Status::Full
                    | crate::sdk::ant_capture::Status::Error
            )
        {
            self.recording_ended.get_or_insert(now);
        }

        if foundation_expired(
            now,
            capture_status,
            self.capture_seconds,
            &mut self.capture_deadline,
        ) {
            for packet in &mut self.sampled_packets {
                if packet.take().is_some() {
                    self.capture.sampled_out_packet();
                }
            }
            self.capture.stop();
            capture_status = self.capture.snapshot().status;
        }
        let link_due = !sampled || now >= self.next_capture_link;
        if sampled
            && link_due
            && matches!(
                capture_status,
                crate::sdk::ant_capture::Status::Ready | crate::sdk::ant_capture::Status::Recording
            )
        {
            self.next_capture_link = now.saturating_add(10_000);
        }
        for (index, channel) in channels.iter().enumerate() {
            let Some(channel_state) = channel else {
                continue;
            };
            let Some(peer) = channel_state.selected else {
                continue;
            };
            if self.capture_started.is_some() || self.foundation_screen {
                if self.capture_started.is_some()
                    && matches!(
                        capture_status,
                        crate::sdk::ant_capture::Status::Ready
                            | crate::sdk::ant_capture::Status::Recording
                    )
                    && link_due
                    && self.capture_link[index]
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
                if (self.capture_started.is_none()
                    || matches!(
                        capture_status,
                        crate::sdk::ant_capture::Status::Scanning
                            | crate::sdk::ant_capture::Status::Ready
                            | crate::sdk::ant_capture::Status::Recording
                    ))
                    && !self.scan.active()
                    && !self.dropped_ant.contains(&Some(peer.device_type))
                    && channel_state.link == crate::ant::LinkState::Disconnected
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
                if self.sampled_packets[index].take().is_some() {
                    self.capture.sampled_out_packet();
                }
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
                if self.capture_started.is_some() {
                    self.capture.packet(packet);
                }
                continue;
            }
            let Some(index) = [40, 120, 11]
                .iter()
                .position(|kind| *kind == packet.identity.device_type)
            else {
                if sampled {
                    self.capture.sampled_out_packet();
                } else {
                    self.capture.packet(packet);
                }
                continue;
            };
            if sampled {
                if matches!(
                    capture_status,
                    crate::sdk::ant_capture::Status::Ready
                        | crate::sdk::ant_capture::Status::Recording
                ) {
                    if self.sampled_packets[index].replace(packet).is_some() {
                        self.capture.sampled_out_packet();
                    }
                } else if capture_status == crate::sdk::ant_capture::Status::Scanning {
                    self.capture.sampled_out_packet();
                }
            } else {
                self.capture.packet(packet);
            }
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
        if self.capture_started.is_some() {
            if matches!(
                capture_status,
                crate::sdk::ant_capture::Status::Ready | crate::sdk::ant_capture::Status::Recording
            ) && now >= self.next_position
            {
                self.next_position = now.saturating_add(if sampled { 2000 } else { 1000 });
                if sampled {
                    for packet in &mut self.sampled_packets {
                        if let Some(packet) = packet.take() {
                            if now.saturating_sub(packet.received_ms) <= 5000 {
                                self.capture.packet(packet);
                            } else {
                                self.capture.sampled_out_packet();
                            }
                        }
                    }
                }
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
            if operation == Some("MENU") && argument == Some("STATUS") && bound.is_none() {
                let _ = write!(
                    output,
                    "open={} active={} startup={} {:?}",
                    self.menu_open,
                    self.foundation_screen,
                    self.startup_status,
                    self.menu.diagnostics()
                );
                return "OK";
            }
            if matches!(operation, Some("SCREEN" | "MENU")) && bound.is_none() {
                self.workout_ui = false;
                self.preview_seconds = match argument {
                    None => None,
                    Some(value) => match value
                        .parse::<u32>()
                        .ok()
                        .filter(|s| sampled_reservation(*s).is_some())
                    {
                        Some(seconds) => Some(seconds),
                        None => return "INVALID",
                    },
                };
                self.foundation_screen = true;
                self.menu_open = operation == Some("MENU") && self.capture_started.is_none();
                return "OK";
            }
            if operation != Some("LOG") {
                return "INVALID";
            }
            if argument == Some("MANUAL") && bound.is_none() {
                return self.start_capture(now, true, Some((0, 8, true)));
            }
            if matches!(argument, Some("START" | "SAMPLED")) {
                let Some(seconds) = bound.and_then(|value| value.parse::<u32>().ok()) else {
                    return "INVALID";
                };
                let selected = ant
                    .channels(now)
                    .iter()
                    .flatten()
                    .any(|channel| channel.selected.is_some());
                let sampled = argument == Some("SAMPLED");
                let Some(required) = (if sampled {
                    sampled_reservation(seconds)
                } else {
                    foundation_reservation(seconds, selected)
                }) else {
                    return "INVALID";
                };
                return self.capture_command(
                    Some("START"),
                    None,
                    store,
                    now,
                    output,
                    true,
                    Some((seconds, required, sampled)),
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
        runtime.menu.workout();
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
