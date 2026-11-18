//! A plan as simulator calls: each one runs on a [`Sim`] or prints as the
//! Rust that runs it, so a printed scenario does what the fuzzer did.
use super::plan::{Card, Course, Plan, RideAction, Sensor, Setup, Start, Step, Touch};
use crate::bulk::Presence;
use crate::route::{Point, Route};
use crate::sim::{Kept, Sim};
use crate::world::Vehicle;
use device_api::input::Role;
use std::fmt::Write as _;

/// The road a course rides, and the Rust that builds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Road {
    pub route: Route,
    pub rust: String,
}

impl Road {
    pub fn of(course: &Course, track: &mut dyn FnMut(u64) -> Option<Route>) -> Self {
        let center =
            |index: u8| super::plan::CENTERS[usize::from(index) % super::plan::CENTERS.len()];
        match course {
            Course::Loop {
                center: at,
                length_m,
                climb_m,
            } => {
                let (lat, lon) = center(*at);
                let (length, climb) = (f64::from(*length_m), f64::from(*climb_m));
                Self {
                    route: Route::loop_around(Point::new(lat, lon), length, climb),
                    rust: format!(
                        "Route::loop_around(Point::new({lat:?}, {lon:?}), {length:?}, {climb:?})"
                    ),
                }
            }
            Course::Profile { center: at, knots } => {
                let (lat, lon) = center(*at);
                let knots: Vec<(f64, f64)> = knots
                    .iter()
                    .map(|&(d, e)| (f64::from(d), f64::from(e)))
                    .collect();
                let listed: Vec<String> = knots
                    .iter()
                    .map(|(d, e)| format!("({d:?}, {e:?})"))
                    .collect();
                Self {
                    route: Route::profile(Point::new(lat, lon), &knots),
                    rust: format!(
                        "Route::profile(Point::new({lat:?}, {lon:?}), &[{}])",
                        listed.join(", ")
                    ),
                }
            }
            Course::Track { points } => Self::track(Route::track(points)),
            Course::Corpus { pick } => match track(*pick) {
                Some(route) => Self::track(route),
                None => Self::of(&super::plan::empty_corpus_loop(*pick), &mut |_| None),
            },
        }
    }

    fn track(route: Route) -> Self {
        let points: Vec<String> = (route.points_e7().into_iter())
            .map(|(lat, lon)| format!("({lat}, {lon})"))
            .collect();
        Self {
            rust: format!("Route::track(&[{}])", points.join(", ")),
            route,
        }
    }
}

#[cfg(test)]
thread_local! {
    /// A test's fault: every power cut this thread runs panics.
    pub(crate) static POWER_CUTS_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// One call a plan, or the fuzzer's end probe, makes of the simulator.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    FitBulk(Presence),
    WaitUsable,
    SkipSetup,
    AntSensor(Sensor),
    RideRoute(Road),
    RideAt {
        kmh_tenths: u16,
        watts: u16,
        bpm: u8,
        rpm: u8,
    },
    Pair(Vec<Sensor>),
    InitBulk,
    FormatRides,
    KeepPreferences(Kept),
    Advance(u64),
    Press(Role),
    Hold(Role, u64),
    Host(RideAction),
    Touch(Touch),
    Present(Sensor, bool),
    GpsFix(bool),
    Barometer(bool),
    Vehicle(Vehicle),
    Battery {
        percent: u8,
        charging: bool,
    },
    CutPower,
    TornWrite {
        after: u32,
        partial: usize,
    },
    CardRefusal {
        after: u32,
        partial: usize,
    },
    ClearStorageFault,
    /// Run until past the boot page, for at most this long.
    LeaveBootPage(u64),
    /// A console command whose answer nothing reads.
    Command(&'static str),
}

/// A sensor's ANT device type, the number the plan gives it and the name
/// pairing shows it by.
pub fn ant(sensor: Sensor) -> (u8, u16, &'static str) {
    use crate::ant::device_type;
    match sensor {
        Sensor::Heart => (device_type::HEART_RATE, 4242, "HEART RATE"),
        Sensor::Power => (device_type::POWER, 1234, "POWER METER"),
        Sensor::SpeedCadence => (device_type::SPEED_CADENCE, 777, "SPEED/CAD"),
        Sensor::Wheel => (device_type::SPEED, 778, "SPEED"),
        Sensor::Cadence => (device_type::CADENCE, 779, "CADENCE"),
        Sensor::Radar => (device_type::RADAR, 31, "RADAR"),
    }
}

fn device_type_const_name(kind: u8) -> &'static str {
    use crate::ant::device_type;
    match kind {
        device_type::HEART_RATE => "HEART_RATE",
        device_type::POWER => "POWER",
        device_type::SPEED_CADENCE => "SPEED_CADENCE",
        device_type::SPEED => "SPEED",
        device_type::CADENCE => "CADENCE",
        _ => "RADAR",
    }
}

impl Call {
    pub fn run(&self, sim: &mut Sim) {
        match self {
            Self::FitBulk(presence) => sim.fit_bulk(*presence),
            Self::WaitUsable => {
                sim.wait_usable();
            }
            Self::SkipSetup => sim.skip_setup(),
            Self::AntSensor(sensor) => {
                let (kind, number, _) = ant(*sensor);
                sim.ant_sensor(kind, number);
            }
            Self::RideRoute(road) => sim.ride_route(road.route.clone()),
            Self::RideAt {
                kmh_tenths,
                watts,
                bpm,
                rpm,
            } => sim.ride_at(
                f64::from(*kmh_tenths) / 10.0,
                f64::from(*watts),
                f64::from(*bpm),
                f64::from(*rpm),
            ),
            Self::Pair(sensors) => {
                let names: Vec<_> = sensors.iter().map(|s| ant(*s).2).collect();
                sim.pair(&names);
            }
            Self::InitBulk => sim.init_bulk(),
            Self::FormatRides => sim.format_rides(),
            Self::KeepPreferences(kept) => sim.keep_preferences(kept),
            Self::Advance(ms) => sim.advance(*ms),
            Self::Press(role) => sim.press(*role),
            Self::Hold(role, ms) => sim.hold(*role, *ms),
            Self::Host(action) => {
                if !sim.is_off() {
                    sim.command_after_reading_outcome(action.command());
                }
            }
            Self::Touch(Touch::At { x, y }) => sim.finger_at(*x, *y),
            Self::Touch(Touch::Release) => sim.lift_finger(),
            Self::Touch(Touch::Cancel) => sim.cancel_touch(),
            Self::Present(sensor, present) => {
                let (kind, number, _) = ant(*sensor);
                sim.set_present(kind, number, *present);
            }
            Self::GpsFix(fix) => sim.set_gps_fix(*fix),
            Self::Barometer(on) => sim.set_barometer(*on),
            Self::Vehicle(vehicle) => sim.vehicle(*vehicle),
            Self::Battery { percent, charging } => sim.set_battery(*percent, *charging),
            Self::CutPower => {
                #[cfg(test)]
                if POWER_CUTS_PANIC.get() {
                    panic!("an injected panic in a power cut");
                }
                sim.cut_power();
            }
            Self::TornWrite { after, partial } => sim.fail_storage(*after, *partial),
            Self::CardRefusal { after, partial } => sim.fail_bulk(*after, *partial),
            Self::ClearStorageFault => sim.clear_storage_fault(),
            Self::LeaveBootPage(within_ms) => {
                sim.leave_boot_page(*within_ms);
            }
            Self::Command(line) => {
                sim.command(line);
            }
        }
    }

    /// The statement that runs this call on `sim`.
    pub fn rust(&self) -> String {
        let number = |sensor: &Sensor| ant(*sensor).1;
        match self {
            Self::FitBulk(presence) => format!("sim.fit_bulk(Presence::{presence:?});"),
            Self::WaitUsable => "sim.wait_usable();".into(),
            Self::SkipSetup => "sim.skip_setup();".into(),
            Self::AntSensor(sensor) => format!(
                "sim.ant_sensor(device_type::{}, {});",
                device_type_const_name(ant(*sensor).0),
                number(sensor)
            ),
            Self::RideRoute(road) => format!("sim.ride_route({});", road.rust),
            Self::RideAt {
                kmh_tenths,
                watts,
                bpm,
                rpm,
            } => format!(
                "sim.ride_at({}.{}, {watts}.0, {bpm}.0, {rpm}.0);",
                kmh_tenths / 10,
                kmh_tenths % 10
            ),
            Self::Pair(sensors) => {
                let names: Vec<String> =
                    sensors.iter().map(|s| format!("{:?}", ant(*s).2)).collect();
                format!("sim.pair(&[{}]);", names.join(", "))
            }
            Self::InitBulk => "sim.init_bulk();".into(),
            Self::FormatRides => "sim.format_rides();".into(),
            Self::KeepPreferences(kept) => {
                let sensors: Vec<String> = (kept.sensors.iter())
                    .map(|&(kind, number)| {
                        format!("(device_type::{}, {number})", device_type_const_name(kind))
                    })
                    .collect();
                let setup = match kept.setup {
                    Some(step) => format!("Some(SetupStep::{step:?})"),
                    None => "None".into(),
                };
                let profile = kept.profile;
                format!(
                    "sim.keep_preferences(&Kept {{\n        setup: {setup},\n        profile: TrainingProfile {{ ftp_watts: {}, lthr_bpm: {} }},\n        sensors: vec![{}],\n    }});",
                    profile.ftp_watts,
                    profile.lthr_bpm,
                    sensors.join(", ")
                )
            }
            Self::Advance(ms) => format!("sim.advance({ms});"),
            Self::Press(role) => format!("sim.press(Role::{role:?});"),
            Self::Hold(role, ms) => format!("sim.hold(Role::{role:?}, {ms});"),
            Self::Host(action) => format!(
                "if !sim.is_off() {{\n        sim.command_after_reading_outcome({:?});\n    }}",
                action.command()
            ),
            Self::Touch(Touch::At { x, y }) => format!("sim.finger_at({x}, {y});"),
            Self::Touch(Touch::Release) => "sim.lift_finger();".into(),
            Self::Touch(Touch::Cancel) => "sim.cancel_touch();".into(),
            Self::Present(sensor, present) => format!(
                "sim.set_present(device_type::{}, {}, {present});",
                device_type_const_name(ant(*sensor).0),
                number(sensor)
            ),
            Self::GpsFix(fix) => format!("sim.set_gps_fix({fix});"),
            Self::Barometer(on) => format!("sim.set_barometer({on});"),
            Self::Vehicle(vehicle) => format!(
                "sim.vehicle(Vehicle {{ range_m: {:?}, closing_kmh: {:?}, fast: {}, side: Side::{:?} }});",
                vehicle.range_m, vehicle.closing_kmh, vehicle.fast, vehicle.side
            ),
            Self::Battery { percent, charging } => {
                format!("sim.set_battery({percent}, {charging});")
            }
            Self::CutPower => "sim.cut_power();".into(),
            Self::TornWrite { after, partial } => format!("sim.fail_storage({after}, {partial});"),
            Self::CardRefusal { after, partial } => format!("sim.fail_bulk({after}, {partial});"),
            Self::ClearStorageFault => "sim.clear_storage_fault();".into(),
            Self::LeaveBootPage(within_ms) => format!("sim.leave_boot_page({within_ms});"),
            Self::Command(line) => format!("sim.command({line:?});"),
        }
    }
}

/// What the fuzzer does before the first step.
pub fn prepare(setup: &Setup, road: Road) -> Vec<Call> {
    let mut calls = Vec::new();
    if setup.card != Card::None {
        calls.push(Call::FitBulk(match setup.card {
            Card::Ready => Presence::Ready,
            _ => Presence::Missing,
        }));
    }
    let home = setup.start == Start::Home;
    match &setup.start {
        Start::Home => calls.extend([Call::WaitUsable, Call::SkipSetup]),
        Start::PowerOn(kept) => {
            if setup.card == Card::Ready {
                calls.push(Call::FormatRides);
            }
            calls.extend(kept.clone().map(Call::KeepPreferences));
        }
    }
    calls.extend(setup.sensors.iter().map(|s| Call::AntSensor(*s)));
    calls.push(Call::RideRoute(road));
    calls.push(Call::RideAt {
        kmh_tenths: 0,
        watts: 0,
        bpm: 72,
        rpm: 0,
    });
    if !setup.sensors.is_empty() && home {
        calls.push(Call::Pair(setup.sensors.clone()));
    }
    if setup.card == Card::Ready && home {
        calls.push(Call::InitBulk);
    }
    calls
}

/// The call one step makes.
pub fn step(step: &Step) -> Call {
    match *step {
        Step::Wait { ms } => Call::Advance(u64::from(ms)),
        Step::Press(role) => Call::Press(role),
        Step::Hold(role, ms) => Call::Hold(role, u64::from(ms)),
        Step::Ride {
            kmh_tenths,
            watts,
            bpm,
            rpm,
        } => Call::RideAt {
            kmh_tenths,
            watts,
            bpm,
            rpm,
        },
        Step::Stop => Call::RideAt {
            kmh_tenths: 0,
            watts: 0,
            bpm: 90,
            rpm: 0,
        },
        Step::Host(action) => Call::Host(action),
        Step::Dropout { sensor, present } => Call::Present(sensor, present),
        Step::GpsFix(fix) => Call::GpsFix(fix),
        Step::Barometer(on) => Call::Barometer(on),
        Step::Vehicle {
            range_m,
            closing_kmh,
            fast,
            side,
        } => Call::Vehicle(Vehicle {
            range_m: f64::from(range_m),
            closing_kmh: f64::from(closing_kmh),
            fast,
            side,
        }),
        Step::Battery { percent, charging } => Call::Battery { percent, charging },
        Step::Touch(touch) => Call::Touch(touch),
        Step::PowerCut => Call::CutPower,
        Step::TornWrite { after, partial } => Call::TornWrite {
            after: u32::from(after),
            partial: usize::from(partial),
        },
        Step::CardRefusal { after, partial } => Call::CardRefusal {
            after: u32::from(after),
            partial: usize::from(partial),
        },
    }
}

/// Every call `plan` makes on `road`.
pub fn calls(plan: &Plan, road: Road) -> Vec<Call> {
    let mut calls = prepare(&plan.setup, road);
    calls.extend(plan.steps.iter().map(step));
    calls
}

/// `plan` as a test named `name`: its calls, then `then`, the statements
/// that judge the end.
pub fn print(name: &str, calls: &[Call], then: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "#[test]\nfn {name}() {{");
    let statements: Vec<String> = calls.iter().map(Call::rust).collect();
    let uses = |text: &str| statements.iter().any(|s| s.contains(text));
    for (named, import) in [
        ("Role::", "device_api::input::Role"),
        ("device_type::", "shell_simulator::ant::device_type"),
        ("Presence::", "shell_simulator::bulk::Presence"),
        ("Point::", "shell_simulator::route::Point"),
        ("Route::", "shell_simulator::route::Route"),
        ("Side::", "shell_simulator::world::{Side, Vehicle}"),
        ("Kept {", "shell_simulator::sim::Kept"),
        ("SetupStep::", "vana::screens::setup::Step as SetupStep"),
        ("TrainingProfile {", "vana::ride::zones::TrainingProfile"),
    ] {
        if uses(named) {
            let _ = writeln!(out, "    use {import};");
        }
    }
    out.push_str("    let mut sim = shell_simulator::sim::Sim::new();\n");
    for statement in &statements {
        let _ = writeln!(out, "    {statement}");
    }
    let _ = writeln!(out, "{then}}}");
    out
}
