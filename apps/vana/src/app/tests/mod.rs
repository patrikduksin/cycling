//! Runtime tests through its public lifecycle, with deterministic fakes for
//! the capabilities VANA consumes.
use super::*;
use crate::app::navigation::To;
use crate::ride::log::{Commit, REGION_SIZE, SECTOR_SIZE, SECTORS, SLOT_SIZE, Sector, Slot};
use crate::ride::medium::Medium;
use crate::ride::storage::RideStorage;
use device_api::Absent;
use device_api::ant::{AntOperation, Event, Identity};
use device_api::input::Role;
use device_api::observation::{Availability, Observation};
use firmware_shell::input::Input;
use std::vec::Vec;

mod calibration;
mod commands;
mod invariants;
mod pace;
mod phone;
mod power;
mod presentation;
mod recording;
mod saved_sensors;
mod screens;
mod sensor_links;
mod state;
mod sync;
mod touch;

struct Radio {
    channels: firmware_services::ant::Channels,
    requests: Vec<AntOperation>,
    reject: Option<device_api::ant::Error>,
    settling: bool,
    observe_requests: bool,
    availability: Availability,
    /// Disconnects are admitted but their delivery fails, as when the UART
    /// submission to the companion fails.
    fail_disconnects: bool,
}
impl Radio {
    fn new() -> Self {
        Self {
            channels: firmware_services::ant::Channels::new(),
            requests: Vec::new(),
            reject: None,
            settling: false,
            observe_requests: true,
            availability: Availability::Ready,
            fail_disconnects: false,
        }
    }

    /// A radio that admits requests but leaves link events to the test.
    fn quiet() -> Self {
        Self {
            observe_requests: false,
            ..Self::new()
        }
    }

    fn connects(&self, peer: Identity) -> usize {
        self.requests
            .iter()
            .filter(|op| matches!(op, AntOperation::Connect(p) if *p == peer))
            .count()
    }

    fn connected(&mut self, device_type: u8, now: u64) -> Identity {
        let peer = Identity {
            device_type,
            device_number: 42,
            transmission_type: 1,
        };
        self.channels.connect(peer, now).unwrap();
        self.channels.receive(Event::Connected(peer), now);
        peer
    }

    fn data(&mut self, device_type: u8, data: [u8; 8], now: u64) {
        self.channels
            .receive(Event::Data { device_type, data }, now);
    }
}
impl Ant for Radio {
    fn take_diagnostic_packet(&mut self) -> Option<device_api::ant::Packet> {
        None
    }
    fn channel(&self, kind: u8, now_ms: u64) -> Option<device_api::ant::Snapshot> {
        device_api::ant::channel(self.channels(now_ms), kind)
    }
    fn availability(&self) -> Availability {
        self.availability
    }
    fn capabilities(&self) -> device_api::ant::Capabilities {
        device_api::ant::Capabilities {
            concurrent_scan: true,
            ..device_api::ant::Capabilities::UNAVAILABLE
        }
    }
    fn scan(&self) -> device_api::ant::ScanSnapshot {
        let mut snapshot = self.channels.scan();
        if self.settling {
            snapshot.state = device_api::ant::ScanState::Stopping;
        }
        snapshot
    }
    fn scanning(&self) -> bool {
        self.channels.scanning()
    }
    fn discoveries(&self) -> [Option<device_api::ant::Discovery>; 8] {
        *self.channels.discoveries()
    }
    fn channels(
        &self,
        now: u64,
    ) -> [Option<device_api::ant::Snapshot>; device_api::ant::CHANNEL_CAPACITY] {
        self.channels.snapshots(now)
    }
    fn take_packet(&mut self) -> Option<device_api::ant::Packet> {
        self.channels.pop_packet()
    }
    fn request(
        &mut self,
        op: AntOperation,
        now: u64,
    ) -> Result<device_api::ant::Admission, device_api::ant::Error> {
        self.requests.push(op);
        if let Some(error) = self.reject {
            return Err(error);
        }
        match self.requests.last().unwrap() {
            AntOperation::Scan(ms) => self
                .channels
                .begin_scan(now, *ms)
                .map(|_| device_api::ant::Admission::Accepted),
            AntOperation::StopScan => self
                .channels
                .stop_scan(now)
                .map(|_| device_api::ant::Admission::Accepted),
            AntOperation::Disconnect(kind) => {
                let peer = self.channels.channel(*kind, now).unwrap().selected.unwrap();
                self.channels.disconnect(*kind, now).unwrap();
                if self.fail_disconnects {
                    self.channels.request_failed(
                        device_api::ant::Request::Disconnect { identity: peer },
                        now,
                    );
                } else if self.observe_requests {
                    self.channels
                        .receive(device_api::ant::Event::Disconnected(peer), now);
                }
                Ok(device_api::ant::Admission::Accepted)
            }
            AntOperation::Connect(peer) => {
                self.channels.connect(*peer, now)?;
                if self.observe_requests {
                    self.channels
                        .receive(device_api::ant::Event::Connected(*peer), now);
                }
                Ok(device_api::ant::Admission::Accepted)
            }
            AntOperation::Send { target, data } => self
                .channels
                .queue_send(*target, *data, now)
                .map(device_api::ant::Admission::SendQueued),
        }
    }

    fn send_status(
        &self,
        id: device_api::ant::OperationId,
    ) -> Option<device_api::ant::SendObservation> {
        self.channels.send_status(id)
    }
}

struct MustNotWrite;
impl RideStorage for MustNotWrite {
    type Error = ();
    fn ride_geometry(&mut self) -> device_api::storage::Geometry {
        crate::ride::log::GEOMETRY
    }
    fn ride_read_sector(&mut self, _: usize, _: &mut Sector) -> Result<(), ()> {
        Err(())
    }
    fn ride_read_slot(&mut self, _: usize, _: &mut Slot) -> Result<(), ()> {
        Err(())
    }
    fn ride_erase_sector(&mut self, _: usize) -> Result<(), ()> {
        panic!("sensor UI must not erase rides")
    }
    fn ride_write_slot(&mut self, _: usize, _: &Slot) -> Result<(), ()> {
        panic!("sensor UI must not write rides")
    }
    fn ride_commit_slot(&mut self, _: usize, _: &Commit) -> Result<(), ()> {
        panic!("sensor UI must not commit rides")
    }
}

impl device_api::bulk::ReadWrite for MustNotWrite {
    fn entropy(&mut self) -> u64 {
        0
    }
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        Err(device_api::bulk::Error::Unsupported)
    }
    fn owned_read(&mut self, _: u64, _: &mut [u8; 512]) -> Result<(), device_api::bulk::Error> {
        Err(device_api::bulk::Error::Unsupported)
    }
    fn owned_write(&mut self, _: u64, _: &[u8; 512]) -> Result<(), device_api::bulk::Error> {
        panic!("no bulk media to write")
    }
}

/// An in-memory ride reservation with NOR semantics: programming clears bits.
struct Journal(Vec<u8>, bool);
impl Journal {
    fn erased() -> Self {
        Self(std::vec![0xff; REGION_SIZE], true)
    }
    /// A memory-backed reservation of `sectors` sectors, which a power-off
    /// loses.
    fn volatile(sectors: usize) -> Self {
        Self(std::vec![0xff; sectors * SECTOR_SIZE], false)
    }
    fn slot(&self, index: usize) -> Slot {
        let mut slot = Slot::default();
        slot.0
            .copy_from_slice(&self.0[index * SLOT_SIZE..(index + 1) * SLOT_SIZE]);
        slot
    }
}
impl RideStorage for Journal {
    type Error = ();
    fn ride_geometry(&mut self) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: self.0.len(),
            durable: self.1,
            ..crate::ride::log::GEOMETRY
        }
    }
    fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), ()> {
        output
            .0
            .copy_from_slice(&self.0[sector * SECTOR_SIZE..(sector + 1) * SECTOR_SIZE]);
        Ok(())
    }
    fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), ()> {
        *output = self.slot(slot);
        Ok(())
    }
    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), ()> {
        self.0[sector * SECTOR_SIZE..(sector + 1) * SECTOR_SIZE].fill(0xff);
        Ok(())
    }
    fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), ()> {
        let stored = &mut self.0[slot * SLOT_SIZE..(slot + 1) * SLOT_SIZE];
        for (stored, new) in stored.iter_mut().zip(data.0) {
            *stored &= new;
        }
        Ok(())
    }
    fn ride_commit_slot(&mut self, slot: usize, commit: &Commit) -> Result<(), ()> {
        let end = (slot + 1) * SLOT_SIZE;
        self.0[end - commit.0.len()..end].copy_from_slice(&commit.0);
        Ok(())
    }
}

impl Runtime {
    fn toast(&mut self, now: u64) -> Option<crate::screens::chrome::Toast> {
        self.navigation.expire_toast(now);
        self.toast_at(now)
    }
}

fn booted(profile: Profile) -> Runtime {
    let mut runtime = Runtime::new(profile, crate::testing::PRODUCT, &crate::screens::C606);
    runtime.format_event(super::format::Event::IntentAtBoot(
        super::intent::Found::Idle,
    ));
    runtime
}

fn press(button: Role) -> Input {
    Input::Press {
        role: button,
        code: 1,
    }
}

/// Every onboard sensor, as the C606 fits them.
const FITTED: device_api::sensors::SensorSet = device_api::sensors::SensorSet {
    barometer: true,
    light: true,
};

fn no_physical_sensors() -> device_api::sensors::Snapshot {
    device_api::sensors::Snapshot {
        pressure: Observation::Unavailable,
        motion: [Observation::Unavailable; 2],
        identity: Observation::Unavailable,
        reports: 0,
        invalid_reports: 0,
        losses: 0,
    }
}

fn tick(runtime: &mut Runtime, store: &mut impl RideStorage, radio: &mut Radio, now: u64) {
    runtime.tick(
        store,
        &mut MustNotWrite,
        now,
        radio,
        &mut Absent,
        Observations {
            position: &Absent,
            network_time: Default::default(),
            input: &Absent,
            physical: no_physical_sensors(),
            fitted: FITTED,
        },
    );
}

fn advance(runtime: &mut Runtime, radio: &mut Radio, now: u64) {
    tick(runtime, &mut MustNotWrite, radio, now);
}

/// A runtime started with these sensors saved, as after a restart.
fn restored(peers: &[Identity]) -> Runtime {
    let mut saved = crate::sensors::saved::Saved::default();
    for peer in peers {
        assert!(saved.select(*peer).is_some());
    }
    let record = preferences::encode(&preferences::Record {
        sensors: saved,
        ..preferences::Record::default()
    });
    let mut runtime = booted(Profile::HeartRate);
    runtime.load_preferences(
        &firmware_shell::preferences::AppPreferences::new(&record).unwrap(),
        firmware_shell::preferences::Source::Current,
    );
    runtime
}

/// A runtime whose recorder finished scanning an erased journal.
fn ready_runtime(journal: &mut Journal, radio: &mut Radio) -> Runtime {
    let mut runtime = booted(Profile::HeartRate);
    for _ in 0..SECTORS {
        tick(&mut runtime, journal, radio, 0);
    }
    assert_eq!(
        runtime.ride.internal().status(),
        crate::ride::log::Status::Ready
    );
    runtime
}

/// Connect `peer` from NEARBY as the rider does: MOVE to it, ACT, and
/// ACT again on the check cue when it replaces a saved sensor.
fn choose_sensor(runtime: &mut Runtime, radio: &mut Radio, peer: device_api::ant::Identity) {
    use crate::screens::sensors::Page;
    radio.channels.begin_scan(0, 1).unwrap();
    radio.channels.receive(
        device_api::ant::Event::Discovery {
            identity: peer,
            rssi: -40,
        },
        0,
    );
    radio.channels.tick(1);
    runtime
        .navigation
        .go(To::Page(crate::screens::workout::Page::Sensors));
    let mut now = 600;
    let mut step = |runtime: &mut Runtime, radio: &mut Radio, button| {
        runtime.input_received(press(button), now, now, radio);
        now += 400;
    };
    runtime.refresh_sensor_menu(radio, 1);
    runtime.menu.show_found();
    for _ in 0..12 {
        if runtime.menu.diagnostics().focus == Some(peer) {
            break;
        }
        step(runtime, radio, Role::Move);
    }
    assert_eq!(
        runtime.menu.diagnostics().focus,
        Some(peer),
        "{peer:?} is listed"
    );
    step(runtime, radio, Role::Act);
    if runtime.menu.diagnostics().page == Page::Replace {
        step(runtime, radio, Role::Act);
    }
}

/// Forget the saved sensor of `kind` as the rider does from `now`: its
/// row, its detail page, the trash cue, then the trash cue on FORGET
/// SENSOR?. Returns when it pressed last.
fn forget_sensor(runtime: &mut Runtime, radio: &mut Radio, kind: u8, mut now: u64) -> u64 {
    use crate::screens::sensors::Page;
    runtime.open_sensors(false, radio, now);
    for _ in 0..12 {
        if runtime.menu.diagnostics().focus.map(|f| f.device_type) == Some(kind) {
            break;
        }
        now += 400;
        runtime.input_received(press(Role::Move), now, now, radio);
    }
    now += 400;
    runtime.input_received(press(Role::Act), now, now, radio);
    assert_eq!(runtime.menu.diagnostics().page, Page::Detail);
    now += crate::screens::sensors::SETTLE_MS;
    let trash = if kind == crate::sensors::ant::device_type::POWER {
        Role::Move
    } else {
        Role::Act
    };
    runtime.input_received(press(trash), now, now, radio);
    assert_eq!(runtime.menu.diagnostics().page, Page::Forget);
    now += crate::screens::sensors::SETTLE_MS;
    runtime.input_received(press(Role::Act), now, now, radio);
    now
}
