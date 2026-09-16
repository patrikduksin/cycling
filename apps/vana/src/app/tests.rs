use super::*;

use device_api::ant::Ant;
use device_api::ant::AntOperation;
use device_api::input::Button;
use device_api::input::Input;
use device_api::observation::Availability;
struct Radio {
    channels: firmware_services::ant::Channels,
    requests: std::vec::Vec<AntOperation>,
    reject: Option<device_api::ant::Error>,
    settling: bool,
    observe_requests: bool,
}
impl Ant for Radio {
    fn availability(&self) -> Availability {
        Availability::Ready
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
        None
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
                if self.observe_requests {
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
            _ => Ok(device_api::ant::Admission::Accepted),
        }
    }
}

#[test]
fn workout_scan_keeps_existing_channels_receiving() {
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 1,
        transmission_type: 1,
    };
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
    };
    radio.channels.connect(peer, 0).unwrap();
    radio
        .channels
        .receive(device_api::ant::Event::Connected(peer), 1);
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Sensors;
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        1000,
        &mut radio,
    );
    assert!(matches!(
        radio.requests.first(),
        Some(AntOperation::Scan(10000))
    ));
    radio.channels.receive(
        device_api::ant::Event::Data {
            device_type: 120,
            data: [0; 8],
        },
        1050,
    );
    assert!(!radio.channels.channel(120, 1100).unwrap().stale);
    assert_eq!(radio.requests.len(), 1);
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
        device_api::ant::LinkState::Connected
    );

    // Explicit DROP must survive later foreground scans. Select the connected
    // channel row (there are no discovery rows in this radio fixture).
    runtime.input(
        Input::Button {
            button: Button::BottomLeft,
            code: 1,
        },
        12000,
        &mut radio,
    );
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        12500,
        &mut radio,
    );
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        12900,
        &mut radio,
    );
    assert!(runtime.dropped_ant.contains(&Some(120)));
    assert_eq!(
        radio.channels.channel(120, 12500).unwrap().link,
        device_api::ant::LinkState::Disconnected
    );
    radio.requests.clear();
    runtime.scan.start(&mut radio, 13000);
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
        device_api::ant::LinkState::Disconnected
    );

    // An explicit choice from discoveries re-enables this device type.
    radio.channels.begin_scan(23900, 1000).unwrap();
    radio.channels.receive(
        device_api::ant::Event::Discovery {
            identity: peer,
            rssi: -40,
        },
        24000,
    );
    radio.channels.tick(24900);
    runtime.menu = crate::screens::sensors::Menu::new();

    runtime.input(
        Input::Button {
            button: Button::BottomLeft,
            code: 1,
        },
        24500,
        &mut radio,
    );
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        25000,
        &mut radio,
    );
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        25400,
        &mut radio,
    );
    assert!(!runtime.dropped_ant.contains(&Some(120)));
    assert_eq!(
        radio.channels.channel(120, 25000).unwrap().link,
        device_api::ant::LinkState::Connected
    );
}

#[test]
fn back_cancels_starting_scan_and_rescan_waits_for_stop_completion() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Sensors;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    runtime.input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.page == crate::screens::workout::Page::Home);
    assert_eq!(radio.scan().state, device_api::ant::ScanState::Stopping);
    runtime.page = crate::screens::workout::Page::Sensors;
    runtime.input(press(Button::BottomRight), 1800, &mut radio);
    assert_eq!(
        radio
            .requests
            .iter()
            .filter(|r| matches!(r, AntOperation::Scan(_)))
            .count(),
        1
    );
    radio.channels.tick(3400);
    runtime.scan.tick(&mut radio, 3400);
    assert_eq!(runtime.scan.message(), b"SCAN CANCELLED");
    runtime.input(press(Button::BottomRight), 3800, &mut radio);
    assert_eq!(
        radio
            .requests
            .iter()
            .filter(|r| matches!(r, AntOperation::Scan(_)))
            .count(),
        2
    );
}

#[test]
fn scan_failure_reports_recovery_and_keeps_back_usable() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: Some(device_api::ant::Error::Unavailable),
        settling: false,
        observe_requests: true,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Sensors;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    assert_eq!(runtime.menu.diagnostics().message, b"SCAN UNAVAILABLE");
    radio.reject = None;
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    radio
        .channels
        .request_failed(device_api::ant::Request::Scan { duration_ms: 10000 }, 1500);
    runtime.scan.tick(&mut radio, 1500);
    assert_eq!(runtime.scan.message(), b"RADIO LOST - REBOOT");
    runtime.input(press(Button::TopLeft), 1800, &mut radio);
    assert!(runtime.page == crate::screens::workout::Page::Home);
    runtime.page = crate::screens::workout::Page::Sensors;
    runtime.input(press(Button::BottomRight), 2200, &mut radio);
    assert_eq!(radio.requests.len(), 2);
    assert_eq!(runtime.menu.diagnostics().message, b"RADIO LOST - REBOOT");
}

#[test]
fn active_scan_cancel_retries_busy_stop_without_disconnecting_sensor() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
    };
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 1,
        transmission_type: 1,
    };
    radio.channels.connect(peer, 0).unwrap();
    radio
        .channels
        .receive(device_api::ant::Event::Connected(peer), 1);
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Sensors;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    radio.channels.receive(
        device_api::ant::Event::Discovery {
            identity: peer,
            rssi: -40,
        },
        1100,
    );
    runtime.scan.tick(&mut radio, 1100);
    assert_eq!(runtime.scan.message(), b"SCANNING...");
    radio.reject = Some(device_api::ant::Error::Busy);
    runtime.input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.page == crate::screens::workout::Page::Home);
    assert_eq!(runtime.scan.message(), b"STOPPING SCAN");
    radio.reject = None;
    runtime.scan.tick(&mut radio, 1500);
    let requests = radio.requests.len();
    runtime.scan.tick(&mut radio, 1600);
    assert_eq!(radio.requests.len(), requests);
    radio.channels.receive(
        device_api::ant::Event::Data {
            device_type: 120,
            data: [0; 8],
        },
        1700,
    );
    assert!(!radio.channel(120, 1700).unwrap().stale);
    radio.channels.tick(3500);
    runtime.scan.tick(&mut radio, 3500);
    assert_eq!(runtime.scan.message(), b"SCAN CANCELLED");
    assert!(
        !radio
            .requests
            .iter()
            .any(|op| matches!(op, AntOperation::Disconnect(_)))
    );
}

#[test]
fn normal_scan_settling_reports_completion_not_cancellation() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Sensors;
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        1000,
        &mut radio,
    );
    radio.channels.tick(11000);
    radio.settling = true;
    runtime.scan.tick(&mut radio, 11000);
    radio.settling = false;
    runtime.scan.tick(&mut radio, 12000);
    assert_eq!(runtime.scan.message(), b"SCAN DONE - PICK");
}

#[test]
fn opening_sensors_starts_discovery_without_a_scan_press() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Home;
    runtime.input(
        Input::Button {
            button: Button::BottomLeft,
            code: 1,
        },
        1000,
        &mut radio,
    );
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        1400,
        &mut radio,
    );
    assert!(matches!(
        radio.requests.as_slice(),
        [AntOperation::Scan(10000)]
    ));
    runtime.input(
        Input::Button {
            button: Button::TopLeft,
            code: 1,
        },
        1800,
        &mut radio,
    );
    assert!(runtime.page == crate::screens::workout::Page::Home);
    assert!(matches!(
        radio.requests.last(),
        Some(AntOperation::StopScan)
    ));
}

#[test]
fn reentry_waits_for_stop_and_busy_scan_start_retries_without_another_press() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Home;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    runtime.input(press(Button::TopLeft), 1800, &mut radio);
    runtime.input(press(Button::BottomRight), 2200, &mut radio);
    assert_eq!(
        radio
            .requests
            .iter()
            .filter(|r| matches!(r, AntOperation::Scan(_)))
            .count(),
        1
    );
    radio.channels.tick(3900);
    radio.reject = Some(device_api::ant::Error::Busy);
    runtime.scan.tick(&mut radio, 3900);
    radio.reject = None;
    runtime.scan.tick(&mut radio, 4000);
    assert_eq!(radio.scan().state, device_api::ant::ScanState::Starting);
    let count = radio.requests.len();
    runtime.scan.tick(&mut radio, 4100);
    assert_eq!(radio.requests.len(), count);
}

#[test]
fn busy_connection_continues_after_leaving_and_waits_for_observed_success() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: Some(device_api::ant::Error::Busy),
        settling: false,
        observe_requests: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 42,
        transmission_type: 1,
    };
    choose_sensor(&mut runtime, &mut radio, peer);
    assert!(radio.channel(120, 1000).is_none());
    runtime.page = crate::screens::workout::Page::Home;
    radio.reject = None;
    advance(&mut runtime, &mut radio, 1400);
    assert_eq!(radio.channel(120, 1400).unwrap().selected, Some(peer));
    assert_eq!(
        radio.channel(120, 1400).unwrap().link,
        device_api::ant::LinkState::Connecting
    );
    let count = radio.requests.len();
    advance(&mut runtime, &mut radio, 1800);
    assert_eq!(radio.requests.len(), count);
}

#[test]
fn same_type_replacement_waits_for_old_disconnect_observation() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: false,
    };
    let old = device_api::ant::Identity {
        device_type: 120,
        device_number: 1,
        transmission_type: 1,
    };
    let replacement = device_api::ant::Identity {
        device_number: 2,
        ..old
    };
    radio.channels.connect(old, 0).unwrap();
    radio
        .channels
        .receive(device_api::ant::Event::Connected(old), 0);
    let mut runtime = Runtime::new(Profile::HeartRate);
    choose_sensor(&mut runtime, &mut radio, replacement);
    assert!(matches!(
        radio.requests.as_slice(),
        [AntOperation::Disconnect(120)]
    ));
    advance(&mut runtime, &mut radio, 1400);
    assert_eq!(radio.channel(120, 1400).unwrap().selected, Some(old));
    assert_eq!(radio.requests.len(), 1);
    radio
        .channels
        .receive(device_api::ant::Event::Disconnected(old), 1800);
    advance(&mut runtime, &mut radio, 1800);
    assert_eq!(
        radio.channel(120, 1800).unwrap().selected,
        Some(replacement)
    );
    assert!(matches!(radio.requests.last(), Some(AntOperation::Connect(p)) if *p == replacement));
}

#[test]
fn busy_connection_expires_and_is_not_replayed_when_radio_recovers() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: Some(device_api::ant::Error::Busy),
        settling: false,
        observe_requests: true,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 42,
        transmission_type: 1,
    };
    choose_sensor(&mut runtime, &mut radio, peer);
    advance(&mut runtime, &mut radio, 26000);
    radio.reject = None;
    advance(&mut runtime, &mut radio, 27000);
    assert!(radio.channel(120, 27000).is_none());
    assert_eq!(radio.requests.len(), 1);
}

#[test]
fn reentry_does_not_start_a_new_scan_ahead_of_a_pending_connection() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: Some(device_api::ant::Error::Busy),
        settling: false,
        observe_requests: true,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 42,
        transmission_type: 1,
    };
    choose_sensor(&mut runtime, &mut radio, peer);
    runtime.page = crate::screens::workout::Page::Home;
    runtime.home_cursor = true;
    radio.reject = None;
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        1400,
        &mut radio,
    );
    assert!(
        !radio
            .requests
            .iter()
            .any(|op| matches!(op, AntOperation::Scan(_)))
    );
}

// Unavailable optional capabilities keep these tests at the public Runtime lifecycle.
struct Absent;
impl device_api::ble_transport::Ble for Absent {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn snapshot(&self) -> device_api::ble_transport::Snapshot {
        Default::default()
    }
    fn take_packet(&mut self) -> Option<device_api::ble_transport::Packet> {
        None
    }
    fn reconnect(&mut self) -> Result<(), device_api::observation::Error> {
        Err(device_api::observation::Error::Unsupported)
    }
}
impl device_api::positioning::Positioning for Absent {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn snapshot(&self, _: u64) -> Option<device_api::positioning::Snapshot> {
        None
    }
}
impl device_api::input::InputObservation for Absent {
    fn snapshot(&self, _: u64) -> Option<device_api::input::InputSnapshot> {
        None
    }
}
impl crate::ride::storage::RideStorage for Absent {
    type Error = ();
    fn ride_read_sector(&mut self, _: usize, _: &mut crate::ride::log::Sector) -> Result<(), ()> {
        Err(())
    }
    fn ride_read_slot(&mut self, _: usize, _: &mut crate::ride::log::Slot) -> Result<(), ()> {
        Err(())
    }
    fn ride_erase_sector(&mut self, _: usize) -> Result<(), ()> {
        panic!("sensor UI must not erase rides")
    }
    fn ride_write_slot(&mut self, _: usize, _: &crate::ride::log::Slot) -> Result<(), ()> {
        panic!("sensor UI must not write rides")
    }
    fn ride_commit_slot(&mut self, _: usize, _: &crate::ride::log::Commit) -> Result<(), ()> {
        panic!("sensor UI must not commit rides")
    }
}
fn advance(runtime: &mut Runtime, radio: &mut Radio, now: u64) {
    use device_api::observation::Observation::Unavailable;
    runtime.tick(
        &mut Absent,
        now,
        radio,
        &mut Absent,
        &Absent,
        false,
        &Absent,
        device_api::sensors::Snapshot {
            pressure: Unavailable,
            motion: [Unavailable; 2],
            identity: Unavailable,
            reports: 0,
            invalid_reports: 0,
            losses: 0,
        },
    );
}

fn choose_sensor(runtime: &mut Runtime, radio: &mut Radio, peer: device_api::ant::Identity) {
    radio.channels.begin_scan(0, 1).unwrap();
    radio.channels.receive(
        device_api::ant::Event::Discovery {
            identity: peer,
            rssi: -40,
        },
        0,
    );
    radio.channels.tick(1);
    runtime.page = crate::screens::workout::Page::Sensors;
    let selected = radio.channels(1).iter().flatten().count();
    let press = |button| Input::Button { button, code: 1 };
    let mut now = if selected == 0 { 600 } else { 0 };
    for _ in 0..=selected {
        runtime.input(press(Button::BottomLeft), now, radio);
        now += 400;
    }
    runtime.input(press(Button::BottomRight), now, radio);
    if selected > 0 {
        runtime.input(press(Button::BottomRight), now + 400, radio);
    }
}
