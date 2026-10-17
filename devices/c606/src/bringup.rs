//! Saved Wi-Fi and BLE connections start in the background. Startup used to
//! wait up to three seconds for each configuration before the first pass of
//! the foreground loop; polling here lets the panel, terminal and VANA run
//! while the radios settle. Composition chooses what is saved; this only
//! sequences the requests.
use device_api::ble_transport::Ble;
use device_api::connectivity::Operation;
use device_api::network::Network;

/// Bound on each configuration wait, as before.
const SETTLE_MS: u64 = 3_000;

#[derive(Clone, Copy)]
enum Step {
    Done,
    /// The configuration request was accepted; connect once it settles.
    Settling {
        deadline: u64,
    },
}

pub struct Bringup {
    wifi: Step,
    ble: Step,
}

impl Bringup {
    /// Submit saved configurations without waiting for them.
    pub fn start(
        now: u64,
        wifi: Option<device_api::connectivity::WifiConfig>,
        network: &mut impl Network,
        ble_selection: Option<device_api::ble_transport::Selection>,
        ble: &mut impl Ble,
    ) -> Self {
        let settling = Step::Settling {
            deadline: now + SETTLE_MS,
        };
        let wifi = match wifi {
            Some(configuration)
                if network
                    .request(device_api::connectivity::WifiOperation::Configure(Some(
                        configuration,
                    )))
                    .is_ok() =>
            {
                settling
            }
            _ => Step::Done,
        };
        let ble = match ble_selection {
            Some(selection)
                if ble
                    .request(device_api::ble_transport::Operation::Select(Some(
                        selection,
                    )))
                    .is_ok() =>
            {
                settling
            }
            _ => Step::Done,
        };
        Self { wifi, ble }
    }

    /// Connect each link once its configuration completed or its wait ran out.
    pub fn poll(&mut self, now: u64, network: &mut impl Network, ble: &mut impl Ble) {
        if let Step::Settling { deadline } = self.wifi {
            let operation = network.control().operation;
            if operation != Operation::Pending || now >= deadline {
                if operation == Operation::Completed {
                    let _ = network.request(device_api::connectivity::WifiOperation::Connect);
                }
                self.wifi = Step::Done;
            }
        }
        if let Step::Settling { deadline } = self.ble {
            if ble.control().operation != Operation::Pending || now >= deadline {
                let _ = ble.request(device_api::ble_transport::Operation::Connect);
                self.ble = Step::Done;
            }
        }
    }
}
