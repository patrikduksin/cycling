use super::*;

#[test]
fn workout_scan_closes_existing_channels_before_scanning() {
    use device_api::ant::Ant;
    use device_api::ant::AntOperation;
    use device_api::input::Button;
    use device_api::input::Input;
    use device_api::observation::Availability;
    struct Radio {
        channels: firmware_services::ant::Channels,
        requests: std::vec::Vec<AntOperation>,
    }
    impl Ant for Radio {
        fn availability(&self) -> Availability {
            Availability::Ready
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
            match self.requests.last().unwrap() {
                AntOperation::Scan(ms) => self
                    .channels
                    .begin_scan(now, *ms)
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
    let peer = device_api::ant::Identity {
        device_type: 120,
        device_number: 1,
        transmission_type: 1,
    };
    let mut radio = Radio {
        channels: firmware_services::ant::Channels::new(),
        requests: std::vec::Vec::new(),
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
        Some(AntOperation::Disconnect(120))
    ));
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
    runtime.scan.start(&mut radio, 13000, &runtime.dropped_ant);
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
