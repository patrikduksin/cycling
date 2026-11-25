//! The devices VANA's passes use. The composition hands them over at boot
//! and reaches them only through this owner: a pass gets each one bound to
//! its recorder, so an unwrapped device cannot reach `vana::app::pass`.
use crate::trace::{Trace, WorldLog};
use c606_firmware as device;
use core::cell::RefCell;
use device_api::ant::AntRecorder;
use device_api::ble_transport::BleRecorder;
use device_api::bulk::ReadWriteRecorder;
use device_api::input::{InputObservation, InputObservationRecorder, InputSnapshot};
use device_api::observation::{Availability, Error, Observation};
use device_api::positioning::{Positioning, PositioningRecorder};
use device_api::power::{PowerControl, PowerControlRecorder};
use device_api::sensors::{Sensors, SensorsRecorder};
use device_api::server::{AccessPointRecorder, ServerRecorder};
use device_api::sound::{Sound, SoundRecorder};

pub struct World {
    ant: device::board::Ant,
    ble: device::board::Ble,
    position: device::board::Positioning,
    bulk: device::board::Bulk,
    sound: device::board::Sound,
    sensors: device::board::Sensors,
    server: device::drivers::exchange::Server,
    access_point: device::board::AccessPoint,
    input: device::board::Input,
    power: device::board::Power,
}

pub struct Console<'a> {
    pub ant: &'a mut device::board::Ant,
    pub ble: &'a mut device::board::Ble,
    pub position: &'a mut device::board::Positioning,
    pub bulk: &'a mut device::board::Bulk,
    pub sound: &'a mut device::board::Sound,
    pub sensors: &'a mut device::board::Sensors,
    pub input: &'a mut device::board::Input,
    pub power: &'a mut device::board::Power,
}

/// What a pass read of the companion, kept while the pass holds the
/// devices.
#[derive(Clone, Copy)]
pub struct CompanionView(Option<InputSnapshot>);

impl InputObservation for CompanionView {
    fn snapshot(&self, _: u64) -> Option<InputSnapshot> {
        self.0
    }
}

/// What a pass read of the position, kept while the pass holds the devices.
#[derive(Clone, Copy)]
pub struct PositionView {
    availability: Availability,
    snapshot: Option<device_api::positioning::Snapshot>,
}

impl Positioning for PositionView {
    fn availability(&self) -> Availability {
        self.availability
    }
    fn snapshot(&self, _: u64) -> Option<device_api::positioning::Snapshot> {
        self.snapshot
    }
}

/// The buzzer, noting each pattern it accepted: the trace names the cue a
/// pattern played for.
pub struct Started<'a, S> {
    sound: S,
    started: &'a mut heapless::Vec<u8, 4>,
}

impl<S: Sound> Sound for Started<'_, S> {
    fn patterns(&self) -> &'static [device_api::sound::Pattern] {
        self.sound.patterns()
    }
    fn snapshot(&self) -> device_api::sound::Snapshot {
        self.sound.snapshot()
    }
    fn play(&mut self, id: u8, now: u64) -> Result<(), Error> {
        let played = self.sound.play(id, now);
        if played.is_ok() {
            let _ = self.started.push(id);
        }
        played
    }
    fn stop(&mut self, now: u64) -> Result<(), Error> {
        self.sound.stop(now)
    }
}

impl World {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ant: device::board::Ant,
        ble: device::board::Ble,
        position: device::board::Positioning,
        bulk: device::board::Bulk,
        sound: device::board::Sound,
        sensors: device::board::Sensors,
        server: device::drivers::exchange::Server,
        access_point: device::board::AccessPoint,
        input: device::board::Input,
        power: device::board::Power,
    ) -> Self {
        Self {
            ant,
            ble,
            position,
            bulk,
            sound,
            sensors,
            server,
            access_point,
            input,
            power,
        }
    }

    /// The devices a pass hands VANA, each sampled for the pass through
    /// its recorder, in the order the simulator's session samples them.
    #[inline(always)]
    #[allow(clippy::type_complexity)]
    pub fn handles<'a>(
        &'a mut self,
        log: &'a RefCell<WorldLog>,
        observed: &'a crate::trace::Observed,
        started: &'a mut heapless::Vec<u8, 4>,
    ) -> vana::app::Handles<
        impl device_api::ant::Ant + 'a,
        impl device_api::ble_transport::Ble + 'a,
        impl device_api::bulk::ReadWrite + 'a,
        impl device_api::server::AccessPoint + 'a,
        impl device_api::server::Server + 'a,
        impl Sound + 'a,
        impl PowerControl + 'a,
        impl device_api::connectivity::Wifi + 'a,
    > {
        let ant = AntRecorder::new(&mut self.ant, log, &observed.ant);
        let ble = BleRecorder::new(&mut self.ble, log, &observed.ble);
        let bulk = ReadWriteRecorder::new(&mut self.bulk, log, &observed.bulk);
        let access_point =
            AccessPointRecorder::new(&mut self.access_point, log, &observed.access_point);
        let server = ServerRecorder::new(&mut self.server, log, &observed.server);
        let sound = SoundRecorder::new(&mut self.sound, log, &observed.sound);
        let control = PowerControlRecorder::new(&mut self.power, log, &observed.control);
        ant.sample();
        ble.sample();
        access_point.sample();
        server.sample();
        sound.sample();
        control.sample();
        vana::app::Handles {
            ant,
            ble,
            bulk,
            access_point,
            server,
            sound: Started { sound, started },
            control,
            wifi: device_api::connectivity::WifiRecorder::new(
                crate::commands::Station,
                log,
                &observed.wifi,
            ),
        }
    }

    /// The card as VANA's storage and a recorded command reach it.
    #[inline(always)]
    pub fn recorded_bulk(
        &mut self,
        log: crate::trace::Journal,
        observed: &'static crate::trace::Observed,
    ) -> impl device_api::bulk::ReadWrite + '_ {
        ReadWriteRecorder::new(&mut self.bulk, log, &observed.bulk)
    }

    /// For the trace's own drain, the host's transfer session and the
    /// console's STORE commands, which the trace does not record.
    pub fn unrecorded_bulk(&mut self) -> &mut device::board::Bulk {
        &mut self.bulk
    }

    pub fn unrecorded_console(&mut self) -> Console<'_> {
        Console {
            ant: &mut self.ant,
            ble: &mut self.ble,
            position: &mut self.position,
            bulk: &mut self.bulk,
            sound: &mut self.sound,
            sensors: &mut self.sensors,
            input: &mut self.input,
            power: &mut self.power,
        }
    }

    #[inline(always)]
    pub fn companion(&self, trace: &Trace, now: u64) -> CompanionView {
        let recorded =
            InputObservationRecorder::new(&self.input, trace.log, &trace.observed.companion);
        CompanionView(recorded.snapshot(now))
    }

    #[inline(always)]
    pub fn position(&self, trace: &Trace, now: u64) -> PositionView {
        let recorded =
            PositioningRecorder::new(&self.position, trace.log, &trace.observed.position);
        PositionView {
            availability: recorded.availability(),
            snapshot: recorded.snapshot(now),
        }
    }

    /// The power status this pass or command reads.
    #[inline(always)]
    pub fn power(&mut self, trace: &Trace) -> device_api::power::Status {
        PowerControlRecorder::new(&mut self.power, trace.log, &trace.observed.control).status()
    }

    #[inline(always)]
    pub fn startup(&mut self, trace: &Trace) -> device_api::sensors::Startup {
        SensorsRecorder::new(&mut self.sensors, trace.log, &trace.observed.sensors).startup_status()
    }

    /// The motion and pressure sensors, the ambient light and the fitted
    /// sensors this pass reads.
    #[inline(always)]
    pub fn sensed(
        &mut self,
        trace: &Trace,
        now: u64,
    ) -> (
        device_api::sensors::Snapshot,
        Observation<device_api::sensors::Light>,
        device_api::sensors::SensorSet,
    ) {
        let sensors = SensorsRecorder::new(&mut self.sensors, trace.log, &trace.observed.sensors);
        (sensors.snapshot(now), sensors.light(now), sensors.fitted())
    }

    pub fn ant(&self) -> &device::board::Ant {
        &self.ant
    }

    pub fn ble(&self) -> &device::board::Ble {
        &self.ble
    }

    pub fn ble_mut(&mut self) -> &mut device::board::Ble {
        &mut self.ble
    }
}
