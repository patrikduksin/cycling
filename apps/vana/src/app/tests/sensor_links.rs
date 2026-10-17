use super::*;
use crate::screens::sensors::MenuMessage;
use device_api::ant::{Ant, AntOperation};
use device_api::input::{Button, Input};

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
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    radio.channels.connect(peer, 0).unwrap();
    radio
        .channels
        .receive(device_api::ant::Event::Connected(peer), 1);
    let mut runtime = restored(&[peer]);
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
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
    runtime.discovery.scan.tick(&mut radio, 1100);
    assert!(matches!(
        radio.requests.last(),
        Some(AntOperation::Scan(10000))
    ));
    radio.channels.tick(11200);
    runtime.discovery.scan.tick(&mut radio, 11200);
    runtime.discovery.scan.tick(&mut radio, 11300);
    runtime.discovery.scan.tick(&mut radio, 11400);
    assert!(!runtime.discovery.scan.active());
    assert_eq!(
        radio.channels.channel(120, 11400).unwrap().link,
        device_api::ant::LinkState::Connected
    );

    // Disconnect must survive later foreground scans. Select the saved tile
    // (there are no discovery rows in this radio fixture).
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
    assert!(runtime.links.saved.of_type(120).is_none());
    assert!(runtime.preferences.sensors.of_type(120).is_none());
    assert_eq!(
        radio.channels.channel(120, 12500).unwrap().link,
        device_api::ant::LinkState::Disconnected
    );
    radio.requests.clear();
    runtime.discovery.scan.start(&mut radio, 13000);
    radio.channels.tick(23200);
    runtime.discovery.scan.tick(&mut radio, 23200);
    runtime.discovery.scan.tick(&mut radio, 23300);
    assert!(!runtime.discovery.scan.active());
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
    assert_eq!(runtime.links.saved.of_type(120), Some((0, peer)));
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
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    runtime.input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.navigation.page == crate::screens::workout::Page::Home);
    assert_eq!(radio.scan().state, device_api::ant::ScanState::Stopping);
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
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
    runtime.discovery.scan.tick(&mut radio, 3400);
    assert_eq!(runtime.discovery.scan.message(), MenuMessage::ScanCancelled);
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
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    assert_eq!(
        runtime.menu.diagnostics().message,
        Some(MenuMessage::ScanUnavailable)
    );
    radio.reject = None;
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    radio
        .channels
        .request_failed(device_api::ant::Request::Scan { duration_ms: 10000 }, 1500);
    runtime.discovery.scan.tick(&mut radio, 1500);
    assert_eq!(runtime.discovery.scan.message(), MenuMessage::RadioLost);
    runtime.input(press(Button::TopLeft), 1800, &mut radio);
    assert!(runtime.navigation.page == crate::screens::workout::Page::Home);
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
    runtime.input(press(Button::BottomRight), 2200, &mut radio);
    assert_eq!(radio.requests.len(), 2);
    assert_eq!(
        runtime.menu.diagnostics().message,
        Some(MenuMessage::RadioLost)
    );
}

#[test]
fn active_scan_cancel_retries_busy_stop_without_disconnecting_sensor() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
        availability: Availability::Ready,
        fail_disconnects: false,
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
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    radio.channels.receive(
        device_api::ant::Event::Discovery {
            identity: peer,
            rssi: -40,
        },
        1100,
    );
    runtime.discovery.scan.tick(&mut radio, 1100);
    assert_eq!(runtime.discovery.scan.message(), MenuMessage::Scanning);
    radio.reject = Some(device_api::ant::Error::Busy);
    runtime.input(press(Button::TopLeft), 1400, &mut radio);
    assert!(runtime.navigation.page == crate::screens::workout::Page::Home);
    assert_eq!(runtime.discovery.scan.message(), MenuMessage::StoppingScan);
    radio.reject = None;
    runtime.discovery.scan.tick(&mut radio, 1500);
    let requests = radio.requests.len();
    runtime.discovery.scan.tick(&mut radio, 1600);
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
    runtime.discovery.scan.tick(&mut radio, 3500);
    assert_eq!(runtime.discovery.scan.message(), MenuMessage::ScanCancelled);
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
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
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
    runtime.discovery.scan.tick(&mut radio, 11000);
    radio.settling = false;
    runtime.discovery.scan.tick(&mut radio, 12000);
    assert_eq!(runtime.discovery.scan.message(), MenuMessage::ScanDone);
}

#[test]
fn opening_sensors_starts_discovery_without_a_scan_press() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: true,
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = crate::screens::workout::Page::Home;
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
    assert!(runtime.navigation.page == crate::screens::workout::Page::Home);
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
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = crate::screens::workout::Page::Home;
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
    runtime.discovery.scan.tick(&mut radio, 3900);
    radio.reject = None;
    runtime.discovery.scan.tick(&mut radio, 4000);
    assert_eq!(radio.scan().state, device_api::ant::ScanState::Starting);
    let count = radio.requests.len();
    runtime.discovery.scan.tick(&mut radio, 4100);
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
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 42,
        transmission_type: 1,
    };
    choose_sensor(&mut runtime, &mut radio, peer);
    assert!(radio.channel(120, 1000).is_none());
    runtime.input(
        Input::Button {
            button: Button::TopLeft,
            code: 1,
        },
        1400,
        &mut radio,
    );
    assert!(runtime.navigation.page == crate::screens::workout::Page::Home);
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
        availability: Availability::Ready,
        fail_disconnects: false,
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
    let mut runtime = restored(&[old]);
    choose_sensor(&mut runtime, &mut radio, replacement);
    // The saved selection changes at once; the old peer closes first.
    assert_eq!(
        runtime.preferences.sensors.of_type(120),
        Some((0, replacement))
    );
    radio.data(120, [0; 8], 1000);
    assert!(runtime.links.rejects(old));
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
fn busy_radio_keeps_the_selection_and_connects_once_it_recovers() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: Some(device_api::ant::Error::Busy),
        settling: false,
        observe_requests: true,
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 42,
        transmission_type: 1,
    };
    choose_sensor(&mut runtime, &mut radio, peer);
    for now in (1000..26_000).step_by(10) {
        advance(&mut runtime, &mut radio, now);
    }
    // Refusals back off to one request every two seconds.
    assert!(radio.connects(peer) <= 16, "{}", radio.connects(peer));
    radio.reject = None;
    for now in (26_000..28_100).step_by(10) {
        advance(&mut runtime, &mut radio, now);
    }
    assert_eq!(
        radio.channel(120, 28_100).unwrap().link,
        device_api::ant::LinkState::Connected
    );
    assert_eq!(runtime.links.saved.of_type(120), Some((0, peer)));
}

#[test]
fn reentry_does_not_start_a_new_scan_ahead_of_a_pending_connection() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: Some(device_api::ant::Error::Busy),
        settling: false,
        observe_requests: true,
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 42,
        transmission_type: 1,
    };
    choose_sensor(&mut runtime, &mut radio, peer);
    runtime.input(
        Input::Button {
            button: Button::TopLeft,
            code: 1,
        },
        1400,
        &mut radio,
    );
    runtime.navigation.home_cursor = 1;
    radio.reject = None;
    runtime.input(
        Input::Button {
            button: Button::BottomRight,
            code: 1,
        },
        1800,
        &mut radio,
    );
    assert!(
        !radio
            .requests
            .iter()
            .any(|op| matches!(op, AntOperation::Scan(_)))
    );
}

#[test]
fn radio_wide_connection_failure_is_visible_on_the_page() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: Some(device_api::ant::Error::Unavailable),
        settling: false,
        observe_requests: false,
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let mut runtime = Runtime::new(Profile::HeartRate);
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 42,
        transmission_type: 1,
    };
    choose_sensor(&mut runtime, &mut radio, peer);
    let tile = runtime.sensor_tiles(&radio, 1000)[0].unwrap();
    assert_eq!(
        tile.connection,
        crate::screens::sensors::Connection::RadioUnavailable
    );
    // Retried no faster than every five seconds.
    for now in (1000..10_000).step_by(10) {
        advance(&mut runtime, &mut radio, now);
    }
    assert!(radio.connects(peer) <= 3, "{}", radio.connects(peer));
}

#[test]
fn busy_reentry_clears_old_discoveries_when_the_new_scan_is_admitted() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: false,
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    radio.channels.begin_scan(0, 1).unwrap();
    for number in 1..=8 {
        radio.channels.receive(
            device_api::ant::Event::Discovery {
                identity: device_api::ant::Identity {
                    device_type: 120,
                    device_number: number,
                    transmission_type: 1,
                },
                rssi: -40,
            },
            0,
        );
    }
    radio.channels.tick(1);
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = crate::screens::workout::Page::Sensors;
    runtime.navigation.home_cursor = 1;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    runtime.input(press(Button::TopLeft), 1400, &mut radio);
    radio.reject = Some(device_api::ant::Error::Busy);
    runtime.input(press(Button::BottomRight), 1800, &mut radio);
    runtime.input(press(Button::BottomLeft), 2200, &mut radio);
    radio.reject = None;
    advance(&mut runtime, &mut radio, 2300);
    let fresh = device_api::ant::Identity {
        device_type: 17,
        device_number: 99,
        transmission_type: 1,
    };
    radio.channels.receive(
        device_api::ant::Event::Discovery {
            identity: fresh,
            rssi: -40,
        },
        2400,
    );
    runtime.input(press(Button::BottomLeft), 2700, &mut radio);
    runtime.input(press(Button::BottomRight), 3100, &mut radio);
    assert_eq!(runtime.menu.selected_identity(17), Some(fresh));
}

#[test]
fn entry_waits_for_an_admitted_connection_before_starting_discovery() {
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
        reject: None,
        settling: false,
        observe_requests: false,
        availability: Availability::Ready,
        fail_disconnects: false,
    };
    let peer = device_api::ant::Identity {
        device_type: 40,
        device_number: 42,
        transmission_type: 1,
    };
    radio.channels.connect(peer, 0).unwrap();
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime.navigation.page = crate::screens::workout::Page::Home;
    let press = |button| Input::Button { button, code: 1 };
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    assert!(
        radio.requests.is_empty(),
        "Do not submit a scan while the channel is connecting"
    );
    radio
        .channels
        .receive(device_api::ant::Event::Connected(peer), 1800);
    advance(&mut runtime, &mut radio, 1800);
    assert!(matches!(
        radio.requests.as_slice(),
        [AntOperation::Scan(10000)]
    ));
}
