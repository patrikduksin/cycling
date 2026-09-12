//! Standard cycling sensor interpretation over generic BLE transport packets.
//! This client has no radio/global state. Composition feeds snapshots and packets.
use super::ble_sensor::{self, CadenceReading, CscMeasurement, HeartRate, Profile};
use crate::ble_transport::{self, Packet, Selection};

/// Explicit profile selection, with no discovery of an arbitrary nearby sensor.
pub fn selection(
    profile: Profile,
    name: &'static [u8],
    address: Option<[u8; 6]>,
) -> Option<Selection> {
    let (service, characteristic) = match profile {
        Profile::Echo => return None,
        Profile::HeartRate => (0x180d, 0x2a37),
        Profile::Cadence => (0x1816, 0x2a5b),
    };
    Some(Selection {
        name,
        address,
        service,
        characteristic,
    })
}

#[cfg(feature = "c606")]
pub fn configured() -> (Option<Selection>, Profile) {
    mod config {
        include!(env!("CYCLING_BLE_CONFIG"));
    }
    let profile = Profile::from_u8(config::PROFILE);
    (
        selection(profile, config::TARGET_NAME, config::TARGET_ADDRESS),
        profile,
    )
}

pub struct Client {
    profile: Profile,
    transport: ble_transport::Snapshot,
    heart: Option<(u16, u64)>,
    cadence: CadenceReading,
    last_sequence: Option<u32>,
    dropped: u32,
    invalid: u32,
    rr_dropped: u32,
}
impl Client {
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            transport: ble_transport::Snapshot::default(),
            heart: None,
            cadence: CadenceReading::default(),
            last_sequence: None,
            dropped: 0,
            invalid: 0,
            rr_dropped: 0,
        }
    }
    fn reset_readings(&mut self) {
        self.heart = None;
        self.cadence.disconnected();
        self.last_sequence = None;
    }
    /// Call with the current transport state before draining its packet queue.
    /// Lost packets or a new/disconnected link invalidate measurement continuity.
    pub fn update(&mut self, transport: ble_transport::Snapshot) {
        if transport.connections != self.transport.connections
            || transport.link != ble_transport::Link::Connected
            || transport.dropped != self.dropped
        {
            self.reset_readings();
        }
        self.dropped = transport.dropped;
        self.transport = transport;
    }
    pub fn packet(&mut self, packet: Packet) {
        if self.transport.link != ble_transport::Link::Connected
            || packet.connection != self.transport.connections
            || packet.dropped < self.dropped
        {
            return;
        }
        if packet.dropped != self.dropped {
            self.reset_readings();
            self.dropped = packet.dropped;
        }
        if let Some(previous) = self.last_sequence {
            if packet.sequence <= previous {
                return;
            }
            if packet.sequence != previous.saturating_add(1) {
                self.reset_readings();
            }
        }
        self.last_sequence = Some(packet.sequence);
        let Some(data) = packet.data() else {
            self.invalid = self.invalid.saturating_add(1);
            return;
        };
        match self.profile {
            Profile::Echo => {}
            Profile::HeartRate => match HeartRate::parse(data) {
                Some(value) => {
                    self.rr_dropped = self.rr_dropped.saturating_add(u32::from(value.rr_dropped));
                    self.heart =
                        ble_sensor::usable_heart(value).map(|bpm| (bpm, packet.received_ms));
                }
                None => self.invalid = self.invalid.saturating_add(1),
            },
            Profile::Cadence => match CscMeasurement::parse(data) {
                Some(value) => {
                    if let Some((revolutions, event_time)) = value.crank {
                        self.cadence
                            .update(revolutions, event_time, packet.received_ms);
                    }
                }
                None => self.invalid = self.invalid.saturating_add(1),
            },
        }
    }
    pub fn snapshot(&self, now: u64) -> ble_sensor::Snapshot {
        let (heart_bpm, heart_age_ms) = self
            .heart
            .map(|(value, at)| ble_sensor::fresh_u16(value, at, now))
            .unwrap_or((None, None));
        let (cadence_tenths, cadence_age_ms) = self.cadence.snapshot(now);
        let link = match self.transport.link {
            ble_transport::Link::Off | ble_transport::Link::Advertising => ble_sensor::Link::Off,
            ble_transport::Link::Scanning => ble_sensor::Link::Scanning,
            ble_transport::Link::Connecting => ble_sensor::Link::Connecting,
            ble_transport::Link::Connected => ble_sensor::Link::Connected,
            ble_transport::Link::Retrying => ble_sensor::Link::Retrying,
            ble_transport::Link::Failed => ble_sensor::Link::Failed,
        };
        ble_sensor::Snapshot {
            profile: self.profile,
            link,
            heart_bpm,
            heart_age_ms,
            cadence_tenths,
            cadence_age_ms,
            connections: self.transport.connections,
            disconnections: self.transport.disconnections,
            notifications: self.transport.notifications,
            invalid: self.invalid,
            rr_dropped: self.rr_dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(connection: u32, dropped: u32) -> ble_transport::Snapshot {
        ble_transport::Snapshot {
            link: ble_transport::Link::Connected,
            connections: connection,
            dropped,
            ..Default::default()
        }
    }
    fn packet(connection: u32, sequence: u32, dropped: u32, ms: u64, data: &[u8]) -> Packet {
        let mut bytes = [0; 60];
        bytes[..data.len()].copy_from_slice(data);
        Packet {
            connection,
            sequence,
            dropped,
            received_ms: ms,
            bytes,
            length: data.len() as u8,
        }
    }
    #[test]
    fn contact_freshness_invalid_and_disconnect_remain_distinct() {
        let mut client = Client::new(Profile::HeartRate);
        client.update(state(1, 0));
        client.packet(packet(1, 1, 0, 100, &[0, 72]));
        assert_eq!(client.snapshot(5100).heart_bpm, Some(72));
        assert_eq!(client.snapshot(5101).heart_bpm, None);
        client.packet(packet(1, 2, 0, 5101, &[0x04, 72]));
        assert_eq!(client.snapshot(5101).heart_bpm, None);
        client.packet(packet(1, 3, 0, 5102, &[0x01]));
        assert_eq!(client.snapshot(5102).invalid, 1);
        client.packet(packet(1, 4, 0, 5103, &[0, 73]));
        client.update(ble_transport::Snapshot {
            link: ble_transport::Link::Retrying,
            ..state(1, 0)
        });
        assert_eq!(client.snapshot(5103).heart_bpm, None);
    }
    #[test]
    fn connection_generation_and_dropped_packets_reject_old_measurements() {
        let mut client = Client::new(Profile::HeartRate);
        client.update(state(2, 1));
        client.packet(packet(1, 1, 1, 0, &[0, 72]));
        client.packet(packet(2, 2, 0, 0, &[0, 72]));
        assert_eq!(client.snapshot(0).heart_bpm, None);
        client.packet(packet(2, 3, 1, 1, &[0, 73]));
        assert_eq!(client.snapshot(1).heart_bpm, Some(73));
        client.update(state(2, 2));
        assert_eq!(client.snapshot(1).heart_bpm, None);
    }
    #[test]
    fn cadence_does_not_bridge_lost_packets_or_new_connections() {
        let mut client = Client::new(Profile::Cadence);
        client.update(state(1, 0));
        client.packet(packet(1, 1, 0, 1000, &[2, 1, 0, 0, 4]));
        client.packet(packet(1, 2, 0, 2000, &[2, 2, 0, 0, 8]));
        assert_eq!(client.snapshot(2000).cadence_tenths, Some(600));
        client.packet(packet(1, 4, 0, 4000, &[2, 4, 0, 0, 16]));
        assert_eq!(client.snapshot(4000).cadence_tenths, None);
        client.packet(packet(1, 5, 0, 5000, &[2, 5, 0, 0, 20]));
        assert_eq!(client.snapshot(5000).cadence_tenths, Some(600));
        client.update(state(2, 0));
        client.packet(packet(2, 6, 0, 6000, &[2, 6, 0, 0, 24]));
        assert_eq!(client.snapshot(6000).cadence_tenths, None);
    }
    #[test]
    fn profile_selection_is_explicit_and_preserves_uuid_pairs() {
        assert!(selection(Profile::Echo, b"", None).is_none());
        let heart = selection(Profile::HeartRate, b"fixture", None).unwrap();
        assert_eq!((heart.service, heart.characteristic), (0x180d, 0x2a37));
        let csc = selection(Profile::Cadence, b"fixture", None).unwrap();
        assert_eq!((csc.service, csc.characteristic), (0x1816, 0x2a5b));
    }
}
