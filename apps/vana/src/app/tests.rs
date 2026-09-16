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
                self.channels
                    .receive(device_api::ant::Event::Disconnected(peer), now);
                Ok(device_api::ant::Admission::Accepted)
            }
            AntOperation::Connect(peer) => {
                self.channels.connect(*peer, now)?;
                self.channels
                    .receive(device_api::ant::Event::Connected(*peer), now);
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
    };
    radio.channels.connect(peer, 0).unwrap();
    radio
        .channels
        .receive(device_api::ant::Event::Connected(peer), 1);
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Scan;
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
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Scan;
    let press = |button| Input::Button { button, code: 1 };
    runtime.workout_input(press(Button::BottomRight), 1000, &mut radio);
    runtime.workout_input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.page == crate::screens::workout::Page::Sensors);
    assert_eq!(radio.scan().state, device_api::ant::ScanState::Stopping);
    runtime.page = crate::screens::workout::Page::Scan;
    runtime.workout_input(press(Button::BottomRight), 1800, &mut radio);
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
    runtime.workout_input(press(Button::BottomRight), 3800, &mut radio);
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
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Scan;
    let press = |button| Input::Button { button, code: 1 };
    runtime.workout_input(press(Button::BottomRight), 1000, &mut radio);
    assert_eq!(runtime.menu.diagnostics().message, b"SCAN UNAVAILABLE");
    radio.reject = None;
    runtime.workout_input(press(Button::BottomRight), 1400, &mut radio);
    radio
        .channels
        .request_failed(device_api::ant::Request::Scan { duration_ms: 10000 }, 1500);
    runtime.scan.tick(&mut radio, 1500);
    assert_eq!(runtime.scan.message(), b"RADIO LOST - REBOOT");
    runtime.workout_input(press(Button::TopLeft), 1800, &mut radio);
    assert!(runtime.page == crate::screens::workout::Page::Sensors);
    runtime.page = crate::screens::workout::Page::Scan;
    runtime.workout_input(press(Button::BottomRight), 2200, &mut radio);
    assert_eq!(radio.requests.len(), 2);
    assert_eq!(runtime.menu.diagnostics().message, b"RADIO LOST - REBOOT");
}

#[test]
fn connection_request_reports_busy_unavailable_or_connecting() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
    };
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 1,
        transmission_type: 1,
    };
    radio.channels.begin_scan(0, 1000).unwrap();
    radio.channels.receive(
        device_api::ant::Event::Discovery {
            identity: peer,
            rssi: -40,
        },
        1,
    );
    radio.channels.tick(1000);
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Scan;
    let press = |button| Input::Button { button, code: 1 };
    runtime.workout_input(press(Button::BottomLeft), 1000, &mut radio);
    radio.reject = Some(device_api::ant::Error::Busy);
    runtime.workout_input(press(Button::BottomRight), 1400, &mut radio);
    assert_eq!(runtime.menu.diagnostics().message, b"BUSY - TRY AGAIN");
    radio.reject = Some(device_api::ant::Error::Unavailable);
    runtime.workout_input(press(Button::BottomRight), 1800, &mut radio);
    assert_eq!(runtime.menu.diagnostics().message, b"RADIO UNAVAILABLE");
    radio.reject = None;
    runtime.workout_input(press(Button::BottomRight), 2200, &mut radio);
    assert_eq!(runtime.menu.diagnostics().message, b"CONNECTING...");
}

#[test]
fn active_scan_cancel_retries_busy_stop_without_disconnecting_sensor() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
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
    runtime.page = crate::screens::workout::Page::Scan;
    let press = |button| Input::Button { button, code: 1 };
    runtime.workout_input(press(Button::BottomRight), 1000, &mut radio);
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
    runtime.workout_input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.page == crate::screens::workout::Page::Sensors);
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
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.page = crate::screens::workout::Page::Scan;
    runtime.workout_input(
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
