//! A world slice lifted to a [`Plan`]: the sensors it heard, the track its
//! fixes rode, and its inputs in time order as plan steps. The lift is
//! lossy: ANT pages become `ride_at` values, a GNSS fix's speed the
//! riding speed, and what has no step is counted in [`Lifted::left_out`].
//! Whether the plan still shows the failure is for the caller to run.
use super::plan::{Card, Course, Plan, RideAction, Sensor, Setup, Step};
use crate::route::{Route, distance_m, from_e7};
use crate::trace::world::{self, World};
use device_api::ant;
use device_api::gps::FixState;
use device_api::input::{Controls, Input, Role};
use firmware_services::trace::replayed::Replayed;
use firmware_services::trace::wire::Reader;
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
    kmh: u8,
    watts: u16,
    bpm: u8,
    rpm: u8,
}

struct Lift {
    steps: Vec<Step>,
    clock_ms: u64,
    riding: Riding,
    emitted: Riding,
    emitted_ms: Option<u64>,
    sensors: Vec<Sensor>,
    last_packet: BTreeMap<u8, u64>,
    absent: Vec<Sensor>,
    fix: Option<bool>,
    track: Vec<(i32, i32)>,
    controls: Option<Controls>,
    held: Option<(Role, u64, u64)>,
    card: Card,
    left_out: BTreeMap<String, usize>,
}

fn sensor(device_type: u8) -> Option<Sensor> {
    use crate::ant::device_type as kind;
    match device_type {
        kind::HEART_RATE => Some(Sensor::Heart),
        kind::POWER => Some(Sensor::Power),
        kind::SPEED_CADENCE | kind::SPEED | kind::CADENCE => Some(Sensor::SpeedCadence),
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
        let due = self
            .emitted_ms
            .is_none_or(|at| now_ms >= at + RIDE_EVERY_MS);
        if due && self.riding != self.emitted {
            let Riding {
                kmh,
                watts,
                bpm,
                rpm,
            } = self.riding;
            self.at(
                now_ms,
                Step::Ride {
                    kmh,
                    watts,
                    bpm,
                    rpm,
                },
            );
            self.emitted = self.riding;
            self.emitted_ms = Some(now_ms);
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
            Sensor::Radar => self.left("radar targets"),
            _ => {}
        }
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
        if let Some(mm_s) = gps.speed_mm_s.filter(|_| fresh) {
            self.riding.kmh = (f64::from(mm_s) * 3.6 / 1000.0).round().min(255.0) as u8;
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
        emitted_ms: None,
        sensors: Vec::new(),
        last_packet: BTreeMap::new(),
        absent: Vec::new(),
        fix: None,
        track: Vec::new(),
        controls: None,
        held: None,
        card: Card::None,
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
                match world::read(&call) {
                    Some(World::AntPacket(packet)) => lift.packet(now_ms, &packet),
                    Some(World::Position(Some(snapshot))) => lift.position(now_ms, &snapshot),
                    Some(World::Edge(edge)) => match edge.input {
                        Input::Button { id, .. } => lift.button(now_ms, id),
                        _ => lift.left("touch"),
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
    sensors.sort_by_key(|s| Sensor::ALL.iter().position(|a| a == s));
    Ok(Lifted {
        plan: Plan {
            seed: 0,
            setup: Setup {
                sensors,
                card: lift.card,
                course,
                from_power_on: true,
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
