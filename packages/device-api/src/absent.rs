//! Hardware a device does not have.
//!
//! A board writes `Absent` in the field of a part it lacks, so "not fitted"
//! sits where a reader looks. Every answer is the contract's own word for
//! absence: `Availability::Unsupported`, an empty `Sensors::fitted()`, no
//! patterns, no power operations. None of them is a temporary failure.
use crate::ant::{self, Ant, AntOperation};
use crate::ble_transport::{self, Ble};
use crate::bulk;
use crate::connectivity::{
    ControlStatus, NetworkDiscovery, Wifi, WifiOperation, WifiState, WifiStats,
};
use crate::input::{InputObservation, InputSnapshot};
use crate::observation::{Availability, Error, Observation};
use crate::positioning::{self, Positioning};
use crate::power::{self, Power, PowerControl};
use crate::sensors::{self, SensorSet, Sensors};
use crate::server::{
    AccessPoint, AccessPointConfig, AccessPointState, Listen, PhoneLink, Reach, Request, Server,
};
use crate::sound::{self, Sound};
use crate::storage::{AccessError, Geometry, OwnedFlash, Region};
use core::convert::Infallible;

/// A part that is not fitted. See the module docs for what it answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Absent;

impl Ant for Absent {
    fn scanning(&self) -> bool {
        false
    }
    fn discoveries(&self) -> [Option<ant::Discovery>; 8] {
        [None; 8]
    }
    fn request(&mut self, _: AntOperation, _: u64) -> Result<ant::Admission, ant::Error> {
        Err(ant::Error::Unavailable)
    }
    fn capabilities(&self) -> ant::Capabilities {
        ant::Capabilities::UNAVAILABLE
    }
    fn scan(&self) -> ant::ScanSnapshot {
        ant::ScanSnapshot {
            state: ant::ScanState::Idle,
            generation: 0,
            discovery_overflows: 0,
        }
    }
    fn send_status(&self, _: ant::OperationId) -> Option<ant::SendObservation> {
        None
    }
    fn take_diagnostic_packet(&mut self) -> Option<ant::Packet> {
        None
    }
    fn channel(&self, _: u8, _: u64) -> Option<ant::Snapshot> {
        None
    }
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn channels(&self, _: u64) -> [Option<ant::Snapshot>; ant::CHANNEL_CAPACITY] {
        [None; ant::CHANNEL_CAPACITY]
    }
    fn take_packet(&mut self) -> Option<ant::Packet> {
        None
    }
}

impl Ble for Absent {
    fn request(&mut self, _: ble_transport::Operation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn control(&self) -> ControlStatus {
        ControlStatus::new()
    }
    fn discoveries(&self) -> [Option<ble_transport::Discovery>; 8] {
        [None; 8]
    }
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn snapshot(&self) -> ble_transport::Snapshot {
        Default::default()
    }
    fn take_packet(&mut self) -> Option<ble_transport::Packet> {
        None
    }
    fn take_battery(&mut self) -> Option<ble_transport::BatteryLevel> {
        None
    }
}

impl Wifi for Absent {
    fn request(&mut self, _: WifiOperation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn control(&self) -> ControlStatus {
        ControlStatus::new()
    }
    fn discoveries(&self) -> [Option<NetworkDiscovery>; 8] {
        [None; 8]
    }
    fn state(&self) -> WifiState {
        WifiState::Unconfigured
    }
    fn online(&self) -> bool {
        false
    }
    fn stats(&self) -> WifiStats {
        WifiStats::default()
    }
    fn rssi(&self) -> Option<i8> {
        None
    }
}

impl Positioning for Absent {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn snapshot(&self, _: u64) -> Option<positioning::Snapshot> {
        None
    }
}

impl Sensors for Absent {
    fn startup_status(&self) -> sensors::Startup {
        sensors::Startup::Unsupported
    }
    fn startup_reason(&self) -> Option<u8> {
        None
    }
    fn fitted(&self) -> SensorSet {
        SensorSet::NONE
    }
    fn snapshot(&self, _: u64) -> sensors::Snapshot {
        sensors::Snapshot {
            pressure: Observation::Unavailable,
            motion: [Observation::Unavailable; 2],
            identity: Observation::Unavailable,
            reports: 0,
            invalid_reports: 0,
            losses: 0,
        }
    }
    fn light(&self, _: u64) -> Observation<sensors::Light> {
        Observation::Unavailable
    }
    fn identity_status(&self) -> sensors::IdentityQuery {
        sensors::IdentityQuery::Idle
    }
    fn query_identity(&mut self, _: u64) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

impl InputObservation for Absent {
    fn snapshot(&self, _: u64) -> Option<InputSnapshot> {
        None
    }
}

impl PowerControl for Absent {
    fn capabilities(&self) -> power::Capabilities {
        power::Capabilities::UNSUPPORTED
    }
    fn status(&self) -> power::Status {
        power::Status::IDLE
    }
    fn request(&mut self, _: power::Operation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn request_after(&mut self, _: power::Operation, _: u32) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

impl Power for Absent {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        None
    }
    fn brightness(&mut self, _: u8) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

impl Sound for Absent {
    fn patterns(&self) -> &'static [sound::Pattern] {
        &[]
    }
    fn snapshot(&self) -> sound::Snapshot {
        sound::Snapshot {
            state: sound::State::Idle,
            pattern: None,
            requested_ms: 0,
            submitted_ms: None,
            until_ms: 0,
        }
    }
    fn play(&mut self, _: u8, _: u64) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn stop(&mut self, _: u64) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

impl OwnedFlash for Absent {
    type Error = AccessError<Infallible>;
    fn availability(&self, _: Region) -> Availability {
        Availability::Unsupported
    }
    fn geometry(&self, _: Region) -> Geometry {
        Geometry {
            capacity: 0,
            program_size: 1,
            erase_size: 1,
            durable: false,
        }
    }
    fn read(&mut self, _: Region, _: usize, _: &mut [u8]) -> Result<(), Self::Error> {
        Err(AccessError::Unavailable)
    }
    fn program(&mut self, _: Region, _: usize, _: &[u8]) -> Result<(), Self::Error> {
        Err(AccessError::Unavailable)
    }
    fn erase(&mut self, _: Region, _: usize, _: usize) -> Result<(), Self::Error> {
        Err(AccessError::Unavailable)
    }
}

impl bulk::Read for Absent {
    fn info(&self) -> Result<bulk::Info, bulk::Error> {
        Err(bulk::Error::Unsupported)
    }
    fn read(&mut self, _: u64, _: &mut [u8; bulk::SECTOR_SIZE]) -> Result<(), bulk::Error> {
        Err(bulk::Error::Unsupported)
    }
    fn clock(&mut self, _: u32) -> Result<(), bulk::Error> {
        Err(bulk::Error::Unsupported)
    }
    fn recover(&mut self) -> Result<(), bulk::Error> {
        Err(bulk::Error::Unsupported)
    }
}

impl bulk::ReadWrite for Absent {
    fn owned_info(&mut self) -> Result<bulk::OwnedInfo, bulk::Error> {
        Err(bulk::Error::Unsupported)
    }
    fn owned_read(&mut self, _: u64, _: &mut [u8; bulk::SECTOR_SIZE]) -> Result<(), bulk::Error> {
        Err(bulk::Error::Unsupported)
    }
    fn owned_write(&mut self, _: u64, _: &[u8; bulk::SECTOR_SIZE]) -> Result<(), bulk::Error> {
        Err(bulk::Error::Unsupported)
    }
    fn entropy(&mut self) -> u64 {
        0
    }
}

impl AccessPoint for Absent {
    fn start(&mut self, _: AccessPointConfig) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn stop(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn state(&self) -> AccessPointState {
        AccessPointState::Unsupported
    }
    fn reach(&self) -> Reach {
        Reach::default()
    }
    fn entropy(&mut self) -> u32 {
        0
    }
}

impl Server for Absent {
    fn listen(&mut self, _: Listen) {}
    fn request(&self) -> Option<Request> {
        None
    }
    fn read(&mut self, _: u32, _: &mut [u8]) -> usize {
        0
    }
    fn room(&self) -> usize {
        0
    }
    fn write(&mut self, _: u32, _: &[u8]) -> usize {
        0
    }
    fn finish(&mut self, _: u32) {}
    fn link(&mut self) -> PhoneLink {
        PhoneLink::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_with_a_word_for_absence_answers_unsupported() {
        let mut absent = Absent;
        let unsupported = Err(Error::Unsupported);
        assert_eq!(
            Ble::request(&mut absent, ble_transport::Operation::Scan),
            unsupported
        );
        assert_eq!(Wifi::request(&mut absent, WifiOperation::Scan), unsupported);
        assert_eq!(absent.query_identity(0), unsupported);
        assert_eq!(
            PowerControl::request(&mut absent, power::Operation::Shutdown),
            unsupported
        );
        assert_eq!(absent.brightness(0), unsupported);
        assert_eq!(absent.play(0, 0), unsupported);
        assert_eq!(AccessPoint::stop(&mut absent), unsupported);
    }
}
