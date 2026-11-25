//! The simulated Wi-Fi station and the networks in range of it. It scans,
//! takes a configuration and joins as the C606's does: one operation at a
//! time, each finishing after about as long as the hardware takes, and a
//! join that the network refuses says why.
use device_api::connectivity::{
    ControlError, ControlStatus, NetworkDiscovery, Operation, Text, Wifi, WifiConfig,
    WifiOperation, WifiState, WifiStats,
};
use device_api::observation::Error;

const SCAN_MS: u64 = 2_500;
const CONFIGURE_MS: u64 = 50;
const CONNECT_MS: u64 = 3_000;
const DISCONNECT_MS: u64 = 100;

/// A network in range; `password` is `None` for an open one.
#[derive(Clone, Debug)]
pub struct Network {
    pub ssid: String,
    pub rssi: i8,
    pub password: Option<String>,
}

pub struct Station {
    pub control: ControlStatus,
    pub online: bool,
    /// The world's networks, which a power cut leaves where they are.
    pub around: Vec<Network>,
    config: Option<WifiConfig>,
    state: WifiState,
    discoveries: [Option<NetworkDiscovery>; 8],
    /// The operation under way and the time it still takes.
    work: Option<(WifiOperation, u64)>,
    rssi: Option<i8>,
}

impl Default for Station {
    fn default() -> Self {
        Self {
            control: ControlStatus::new(),
            online: false,
            around: Vec::new(),
            config: None,
            state: WifiState::Unconfigured,
            discoveries: [None; 8],
            work: None,
            rssi: None,
        }
    }
}

impl Station {
    pub fn advance(&mut self, ms: u64) {
        let Some((operation, left)) = self.work else {
            return;
        };
        if left > ms {
            self.work = Some((operation, left - ms));
            return;
        }
        self.work = None;
        let error = match operation {
            WifiOperation::Scan => {
                self.discoveries = [None; 8];
                for (slot, network) in self.discoveries.iter_mut().zip(&self.around) {
                    *slot = Text::new(network.ssid.as_bytes()).map(|ssid| NetworkDiscovery {
                        ssid,
                        rssi: network.rssi,
                        secure: network.password.is_some(),
                    });
                }
                None
            }
            WifiOperation::Configure(config) => {
                self.config = config;
                None
            }
            WifiOperation::Disconnect => None,
            WifiOperation::Connect => self.join(),
        };
        self.control.operation = match error {
            Some(_) => Operation::Failed,
            None => Operation::Completed,
        };
        self.control.error = error;
    }

    fn join(&mut self) -> Option<ControlError> {
        let Some(config) = self.config else {
            return Some(ControlError::Unconfigured);
        };
        // The strongest access point of the SSID, as the radio picks one.
        let found = self
            .around
            .iter()
            .filter(|network| network.ssid.as_bytes() == config.ssid.bytes())
            .max_by_key(|network| network.rssi);
        let refused = match found {
            None => Some(ControlError::NetworkNotFound),
            // The station joins with WPA2 only.
            Some(Network { password: None, .. }) => Some(ControlError::ConnectFailed),
            Some(Network {
                password: Some(password),
                ..
            }) if password.as_bytes() != config.password.bytes() => {
                Some(ControlError::WrongPassword)
            }
            Some(network) => {
                self.state = WifiState::Verified;
                self.online = true;
                self.rssi = Some(network.rssi);
                None
            }
        };
        if refused.is_some() {
            self.state = WifiState::Failed;
        }
        refused
    }
}

impl Wifi for Station {
    fn request(&mut self, operation: WifiOperation) -> Result<(), Error> {
        if self.control.operation == Operation::Pending {
            return Err(Error::Unavailable);
        }
        self.control.sequence = self.control.sequence.wrapping_add(1);
        self.control.operation = Operation::Pending;
        self.control.error = None;
        let takes = match operation {
            WifiOperation::Scan => SCAN_MS,
            WifiOperation::Configure(_) => CONFIGURE_MS,
            WifiOperation::Connect => CONNECT_MS,
            WifiOperation::Disconnect => DISCONNECT_MS,
        };
        if !matches!(operation, WifiOperation::Scan) {
            // Each leaves the network before it acts, as the C606's does.
            self.online = false;
            self.rssi = None;
            self.state = match operation {
                WifiOperation::Connect if self.config.is_some() => WifiState::Connecting,
                _ => WifiState::Unconfigured,
            };
        }
        self.work = Some((operation, takes));
        Ok(())
    }
    fn control(&self) -> ControlStatus {
        self.control
    }
    fn discoveries(&self) -> [Option<NetworkDiscovery>; 8] {
        self.discoveries
    }
    fn state(&self) -> WifiState {
        self.state
    }
    fn online(&self) -> bool {
        self.online
    }
    fn stats(&self) -> WifiStats {
        WifiStats::default()
    }
    fn rssi(&self) -> Option<i8> {
        self.rssi
    }
}
