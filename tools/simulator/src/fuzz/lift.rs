//! A world slice lifted to a [`Plan`]: what the device kept from before
//! power-on, the sensors it heard, the track its fixes rode, and its
//! inputs in time order as plan steps. The lift is lossy: ANT pages become
//! `ride_at` values, the wheel's revolutions or else a GNSS fix's speed the
//! riding speed, and what has no step is counted in [`Lifted::left_out`].
//! Of the preferences VANA read at boot it keeps only the setup step, the
//! training profile and the saved sensors' kinds, nothing personal or
//! secret: the rest is counted where it differs from VANA's defaults.
//! Whether the plan still shows the failure is for the caller to run.
use super::plan::{Card, Course, Plan, RideAction, Sensor, Setup, Start, Step, Touch};
use crate::route::{Route, distance_m, from_e7};
use crate::sim::Kept;
use crate::trace::world::{self, World};
use device_api::ant;
use device_api::gps::FixState;
use device_api::input::{Controls, Input, Role};
use firmware_services::trace::replayed::Replayed;
use firmware_services::trace::wire::{Reader, Wire};
use firmware_services::trace::{self, Event};
use std::collections::BTreeMap;

const DROPOUT_AFTER_MS: u64 = 3_000;
const REPEAT_MS: u64 = crate::sim::PRESS_GAP_MS / 2;
const _: () = assert!(crate::sim::HOLD_REPEAT_MS < REPEAT_MS);
const LONG_PRESS_MS: u64 = 1_000;
const RIDE_EVERY_MS: u64 = 1_000;
const TRACK_SPACING_M: f64 = 10.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifted {
    pub plan: Plan,
    /// What the slice held that no step gives, by what it was.
    pub left_out: BTreeMap<String, usize>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Riding {
    kmh_tenths: u16,
    watts: u16,
    bpm: u8,
    rpm: u8,
}

#[derive(Clone, Copy)]
struct RideWindow {
    step_index: usize,
    start_ms: u64,
}

impl RideWindow {
    fn contains(&self, now_ms: u64) -> bool {
        now_ms < self.start_ms + RIDE_EVERY_MS
    }
}

#[derive(Clone, Copy)]
struct WheelEvent {
    event_time_1024ths: u16,
    revolutions: u16,
    heard_ms: u64,
}

enum KeptAtBoot {
    NotRead,
    NothingKept,
    Kept(Kept),
}

struct Lift {
    steps: Vec<Step>,
    clock_ms: u64,
    riding: Riding,
    emitted: Riding,
    ride_window: Option<RideWindow>,
    wheel_circumference_mm: u32,
    last_wheel_event: Option<WheelEvent>,
    sensors: Vec<Sensor>,
    last_packet: BTreeMap<u8, u64>,
    absent: Vec<Sensor>,
    fix: Option<bool>,
    track: Vec<(i32, i32)>,
    controls: Option<Controls>,
    held: Option<(Role, u64, u64)>,
    card: Card,
    kept: KeptAtBoot,
    left_out: BTreeMap<String, usize>,
}

fn sensor(device_type: u8) -> Option<Sensor> {
    use crate::ant::device_type as kind;
    match device_type {
        kind::HEART_RATE => Some(Sensor::Heart),
        kind::POWER => Some(Sensor::Power),
        kind::SPEED_CADENCE => Some(Sensor::SpeedCadence),
        kind::SPEED => Some(Sensor::Wheel),
        kind::CADENCE => Some(Sensor::Cadence),
        kind::RADAR => Some(Sensor::Radar),
        _ => None,
    }
}

impl Lift {
    fn at(&mut self, now_ms: u64, step: Step) {
        if now_ms > self.clock_ms {
            let mut gap = now_ms - self.clock_ms;
            while gap > 0 {
                let ms = gap.min(u64::from(u32::MAX));
                self.steps.push(Step::Wait { ms: ms as u32 });
                gap -= ms;
            }
            self.clock_ms = now_ms;
        }
        self.clock_ms += step.nominal_ms();
        self.steps.push(step);
    }

    fn left(&mut self, what: impl Into<String>) {
        *self.left_out.entry(what.into()).or_default() += 1;
    }

    fn pass(&mut self, now_ms: u64) {
        let silent: Vec<Sensor> = (self.last_packet.iter())
            .filter(|(_, last)| now_ms.saturating_sub(**last) > DROPOUT_AFTER_MS)
            .filter_map(|(kind, _)| sensor(*kind))
            .filter(|s| !self.absent.contains(s))
            .collect();
        for gone in silent {
            self.absent.push(gone);
            self.at(
                now_ms,
                Step::Dropout {
                    sensor: gone,
                    present: false,
                },
            );
        }
        if self
            .held
            .is_some_and(|(_, _, last)| now_ms.saturating_sub(last) > REPEAT_MS)
        {
            self.release();
        }
        self.ride(now_ms);
    }

    fn ride(&mut self, now_ms: u64) {
        if self.riding == self.emitted {
            return;
        }
        let Riding {
            kmh_tenths,
            watts,
            bpm,
            rpm,
        } = self.riding;
        let step = Step::Ride {
            kmh_tenths,
            watts,
            bpm,
            rpm,
        };
        self.hold_latest_readings_of_window(now_ms, step);
        self.emitted = self.riding;
    }

    fn hold_latest_readings_of_window(&mut self, now_ms: u64, step: Step) {
        match self.ride_window.filter(|window| window.contains(now_ms)) {
            Some(window) => self.steps[window.step_index] = step,
            None => {
                self.at(now_ms, step);
                self.ride_window = Some(RideWindow {
                    step_index: self.steps.len() - 1,
                    start_ms: now_ms,
                });
            }
        }
    }

    fn packet(&mut self, now_ms: u64, packet: &ant::Packet) {
        let kind = packet.identity.device_type;
        let Some(heard) = sensor(kind) else {
            return self.left(format!("ANT device type {kind}"));
        };
        if !self.sensors.contains(&heard) {
            self.sensors.push(heard);
        }
        self.last_packet.insert(kind, now_ms);
        if let Some(back) = self.absent.iter().position(|s| *s == heard) {
            self.absent.remove(back);
            self.at(
                now_ms,
                Step::Dropout {
                    sensor: heard,
                    present: true,
                },
            );
        }
        let data = packet.data;
        match heard {
            Sensor::Heart => self.riding.bpm = data[7],
            Sensor::Power if data[0] == 0x10 => {
                self.riding.watts = u16::from_le_bytes([data[6], data[7]]);
                if data[3] != 0xff {
                    self.riding.rpm = data[3];
                }
            }
            Sensor::SpeedCadence | Sensor::Wheel => {
                self.wheel_speed_from_event(WheelEvent {
                    event_time_1024ths: u16::from_le_bytes([data[4], data[5]]),
                    revolutions: u16::from_le_bytes([data[6], data[7]]),
                    heard_ms: now_ms,
                });
            }
            Sensor::Radar => self.left("radar targets"),
            _ => {}
        }
    }

    fn wheel_speed_from_event(&mut self, event: WheelEvent) {
        match self.last_wheel_event {
            Some(last) if last.event_time_1024ths == event.event_time_1024ths => {
                if event.heard_ms.saturating_sub(last.heard_ms) > DROPOUT_AFTER_MS {
                    self.riding.kmh_tenths = 0;
                }
                return;
            }
            Some(last) => {
                let seconds = f64::from(
                    event
                        .event_time_1024ths
                        .wrapping_sub(last.event_time_1024ths),
                ) / 1024.0;
                let metres = f64::from(event.revolutions.wrapping_sub(last.revolutions))
                    * f64::from(self.wheel_circumference_mm)
                    / 1000.0;
                self.riding.kmh_tenths = (metres / seconds * 36.0).round() as u16;
            }
            None => {}
        }
        self.last_wheel_event = Some(event);
    }

    fn position(&mut self, now_ms: u64, snapshot: &device_api::positioning::Snapshot) {
        let gps = snapshot.gps;
        let fresh = gps.state == FixState::Fresh;
        if self.fix != Some(fresh) {
            if self.fix.is_some() {
                self.at(now_ms, Step::GpsFix(fresh));
            }
            self.fix = Some(fresh);
        }
        if let Some(mm_s) = gps
            .speed_mm_s
            .filter(|_| fresh && self.last_wheel_event.is_none())
        {
            self.riding.kmh_tenths = (f64::from(mm_s) * 0.036).round() as u16;
        }
        if let (true, Some(lat), Some(lon)) = (fresh, gps.latitude_e7, gps.longitude_e7) {
            let point = from_e7(lat, lon);
            let far = self
                .track
                .last()
                .is_none_or(|&(la, lo)| distance_m(from_e7(la, lo), point) >= TRACK_SPACING_M);
            if far {
                self.track.push((lat, lon));
            }
        }
    }

    fn release(&mut self) {
        let Some((role, from, last)) = self.held.take() else {
            return;
        };
        let held = last - from;
        let step = match held < LONG_PRESS_MS {
            true => Step::Press(role),
            false => Step::Hold(role, held.min(u64::from(u16::MAX)) as u16),
        };
        self.at(from, step);
    }

    fn button(&mut self, now_ms: u64, id: u8) {
        let Some(role) = self.controls.as_ref().and_then(|c| c.buttons.role(id)) else {
            return self.left(format!("button {id} with no role"));
        };
        match self.held {
            Some((held, from, _)) if held == role => self.held = Some((role, from, now_ms)),
            _ => {
                self.release();
                self.held = Some((role, now_ms, now_ms));
            }
        }
    }

    fn touch(&mut self, at_ms: u64, touch: Touch) {
        self.release();
        self.at(at_ms, Step::Touch(touch));
    }

    fn read_boot_settings(&mut self, call: &trace::Call<'_>) {
        let mut r = Reader::new(call.body, call.now_ms);
        let loaded = r.byte() == 0;
        let bytes = <&[u8]>::take(&mut r);
        if !loaded || r.read().is_none() {
            return self.left("unreadable settings");
        }
        let (settings, _) = firmware_shell::preferences::decode(Some(bytes));
        if settings.ble.is_some() {
            self.left("a BLE sensor in the settings");
        }
        let (record, stored) = vana::app::decode_preferences(settings.app.bytes());
        self.kept = match stored {
            vana::app::StoredPreferences::Missing => KeptAtBoot::NothingKept,
            vana::app::StoredPreferences::Outdated | vana::app::StoredPreferences::Foreign => {
                self.left("preferences of another version");
                KeptAtBoot::NothingKept
            }
            vana::app::StoredPreferences::Current => {
                self.wheel_circumference_mm = u32::from(record.options.wheel.size_mm());
                KeptAtBoot::Kept(self.kept(&record))
            }
        };
    }

    fn kept(&mut self, record: &vana::app::PreferencesRecord) -> Kept {
        let mut sensors = Vec::new();
        for (_, peer) in record.sensors.peers() {
            match sensor(peer.device_type) {
                Some(kind) => {
                    let (device_type, number, _) = super::script::ant(kind);
                    sensors.push((device_type, number));
                }
                None => self.left(format!("saved ANT device type {}", peer.device_type)),
            }
        }
        let (fresh, _) = vana::app::decode_preferences(&[]);
        let (now, then) = (record.options, fresh.options);
        for (what, differs) in [
            ("auto pause", now.auto_pause != then.auto_pause),
            ("sounds", now.sounds != then.sounds),
            ("auto lap", now.auto_lap != then.auto_lap),
            ("day and night", now.daylight != then.daylight),
            ("theme", now.theme != then.theme),
            ("time zone", now.zone != then.zone),
            ("wheel size", now.wheel != then.wheel),
            ("rider", now.rider != then.rider),
            ("ride alerts", now.alerts != then.alerts),
            ("sunset alert", now.sunset_alert != then.sunset_alert),
            ("ride pages", now.deck != then.deck),
            ("main page readings", now.main_slots != then.main_slots),
            ("display mode", now.mode != then.mode),
            (
                "auto brightness",
                now.auto_brightness != then.auto_brightness,
            ),
        ] {
            if differs {
                self.left(format!("preference {what}"));
            }
        }
        Kept {
            profile: record.profile,
            sensors,
        }
    }

    fn command(&mut self, now_ms: u64, line: &str) {
        let request = line.splitn(3, ' ').nth(2).unwrap_or(line);
        let action = [
            RideAction::Start,
            RideAction::Pause,
            RideAction::Resume,
            RideAction::Finish,
        ]
        .into_iter()
        .find(|action| action.command() == request);
        match action {
            Some(action) => self.at(now_ms, Step::Host(action)),
            None => self.left(format!(
                "command {}",
                request.split(' ').take(2).collect::<Vec<_>>().join(" ")
            )),
        }
    }
}

/// `slice` lifted to a plan, and what it left out.
pub fn lift(slice: &[u8]) -> Result<Lifted, String> {
    let (_, events) = trace::decode(slice).map_err(|e| e.to_string())?;
    let mut lift = Lift {
        steps: Vec::new(),
        clock_ms: 0,
        riding: Riding::default(),
        emitted: Riding {
            bpm: 72,
            ..Riding::default()
        },
        ride_window: None,
        wheel_circumference_mm: vana::ride::metrics::WHEEL_MM,
        last_wheel_event: None,
        sensors: Vec::new(),
        last_packet: BTreeMap::new(),
        absent: Vec::new(),
        fix: None,
        track: Vec::new(),
        controls: None,
        held: None,
        card: Card::None,
        kept: KeptAtBoot::NotRead,
        left_out: BTreeMap::new(),
    };
    let mut now_ms = 0;
    for event in events {
        match event {
            Event::Pass { now_ms: at } => {
                now_ms = at;
                lift.pass(now_ms);
            }
            Event::Command(line) => lift.command(now_ms, line),
            Event::Call(call) => {
                let named =
                    crate::twin::script::declared(call.method).map(|(c, m)| (c.name, m.name));
                if named == Some(("InputSource", "controls")) {
                    let mut r = Reader::new(call.body, call.now_ms);
                    let controls = Controls::replayed(&mut r);
                    lift.controls = r.read().map(|_| controls);
                    continue;
                }
                if named == Some(("Settings", "load")) {
                    if matches!(lift.kept, KeptAtBoot::NotRead) {
                        lift.read_boot_settings(&call);
                    }
                    continue;
                }
                match world::read(&call) {
                    Some(World::AntPacket(packet)) => lift.packet(now_ms, &packet),
                    Some(World::Position(Some(snapshot))) => lift.position(now_ms, &snapshot),
                    Some(World::Edge(edge)) => match edge.input {
                        Input::Button { id, .. } => lift.button(now_ms, id),
                        Input::Touch(point) => lift.touch(
                            edge.received_ms,
                            Touch::At {
                                x: point.x,
                                y: point.y,
                            },
                        ),
                        Input::Release => lift.touch(edge.received_ms, Touch::Release),
                        Input::Cancel => lift.touch(edge.received_ms, Touch::Cancel),
                    },
                    Some(World::NoCard(_)) => lift.card = Card::None,
                    _ if named == Some(("ReadWrite", "owned_info")) => lift.card = Card::Ready,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    lift.release();
    lift.ride(now_ms);
    if now_ms > lift.clock_ms {
        lift.steps.push(Step::Wait {
            ms: (now_ms - lift.clock_ms).min(u64::from(u32::MAX)) as u32,
        });
    }
    let course = match lift.track.len() {
        0 | 1 => {
            lift.left("no GPS track");
            Course::Loop {
                center: 0,
                length_m: 5_000,
                climb_m: 0,
            }
        }
        _ => Course::Track { points: lift.track },
    };
    let mut sensors = lift.sensors;
    sensors.sort();
    Ok(Lifted {
        plan: Plan {
            seed: 0,
            setup: Setup {
                sensors,
                card: lift.card,
                course,
                start: Start::PowerOn(match lift.kept {
                    KeptAtBoot::Kept(kept) => Some(kept),
                    KeptAtBoot::NotRead | KeptAtBoot::NothingKept => None,
                }),
            },
            steps: lift.steps,
        },
        left_out: lift.left_out,
    })
}

/// A course of the same length as `course`'s track, flat, at a made-up
/// place: no geography of the ride is left. A track's fixes carry no
/// elevation, so the grade is not kept.
pub fn synthetic(course: &Course) -> Course {
    let length_m = match course {
        Course::Track { points } => Route::track(points).length_m(),
        _ => return course.clone(),
    };
    Course::Profile {
        center: 0,
        knots: vec![(0, 0), (length_m.round().max(1.0) as u32, 0)],
    }
}
