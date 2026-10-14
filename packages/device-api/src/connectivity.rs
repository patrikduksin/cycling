//! Bounded connectivity configuration and control observations. No credentials in Debug.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: u8,
}
impl<const N: usize> Text<N> {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub fn new(value: &[u8]) -> Option<Self> {
        if value.len() > N
            || value.len() > 255
            || core::str::from_utf8(value).is_err()
            || value.contains(&0)
        {
            return None;
        }
        let mut result = Self::empty();
        result.bytes[..value.len()].copy_from_slice(value);
        result.len = value.len() as u8;
        Some(result)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
    pub fn text(&self) -> &str {
        core::str::from_utf8(self.bytes()).unwrap()
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
impl<const N: usize> core::fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("<private>")
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiConfig {
    pub ssid: Text<32>,
    pub password: Text<63>,
    pub wpa3: bool,
}
impl WifiConfig {
    pub fn new(ssid: &[u8], password: &[u8], wpa3: bool) -> Option<Self> {
        if ssid.is_empty() || !(8..=63).contains(&password.len()) {
            return None;
        }
        Some(Self {
            ssid: Text::new(ssid)?,
            password: Text::new(password)?,
            wpa3,
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Idle,
    Pending,
    Completed,
    Failed,
}
impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "error",
        }
    }
}
/// Why the most recent connectivity operation failed. Terminal replies print
/// `name()`, so each word is part of the wire format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    Unconfigured,
    ScanFailed,
    ScanTimeout,
    PeerNotFound,
    ConnectFailed,
    ConnectTimeout,
    ConnectionEnded,
    GattFailed,
    DisconnectFailed,
    DisconnectTimeout,
    ConfigurationFailed,
    LinkLost,
    RunnerStopped,
}
impl ControlError {
    pub const ALL: [Self; 13] = [
        Self::Unconfigured,
        Self::ScanFailed,
        Self::ScanTimeout,
        Self::PeerNotFound,
        Self::ConnectFailed,
        Self::ConnectTimeout,
        Self::ConnectionEnded,
        Self::GattFailed,
        Self::DisconnectFailed,
        Self::DisconnectTimeout,
        Self::ConfigurationFailed,
        Self::LinkLost,
        Self::RunnerStopped,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::ScanFailed => "scan_failed",
            Self::ScanTimeout => "scan_timeout",
            Self::PeerNotFound => "peer_not_found",
            Self::ConnectFailed => "connect_failed",
            Self::ConnectTimeout => "connect_timeout",
            Self::ConnectionEnded => "connection_ended",
            Self::GattFailed => "gatt_failed",
            Self::DisconnectFailed => "disconnect_failed",
            Self::DisconnectTimeout => "disconnect_timeout",
            Self::ConfigurationFailed => "configuration_failed",
            Self::LinkLost => "link_lost",
            Self::RunnerStopped => "runner_stopped",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlStatus {
    pub sequence: u32,
    pub operation: Operation,
    pub error: Option<ControlError>,
}
impl ControlStatus {
    pub const fn new() -> Self {
        Self {
            sequence: 0,
            operation: Operation::Idle,
            error: None,
        }
    }
    /// Wire word for the last error; `none` when the operation did not fail.
    pub const fn error_name(&self) -> &'static str {
        match self.error {
            Some(error) => error.name(),
            None => "none",
        }
    }
}
impl Default for ControlStatus {
    fn default() -> Self {
        Self::new()
    }
}
#[derive(Clone, Copy, Debug)]
pub enum WifiOperation {
    Scan,
    Configure(Option<WifiConfig>),
    Connect,
    Disconnect,
}
/// Public station state. `code()` is the number terminal replies report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiState {
    Unconfigured,
    Connecting,
    /// Associated, waiting for DHCP.
    Associated,
    /// DHCP configured, connectivity check pending.
    AddressReady,
    /// Connectivity check succeeded on this association.
    Verified,
    /// Association lost or a check failed; recovery is in progress.
    Retrying,
    Failed,
}
impl WifiState {
    pub const fn code(self) -> u8 {
        match self {
            Self::Unconfigured => 0,
            Self::Connecting => 1,
            Self::Associated => 2,
            Self::AddressReady => 3,
            Self::Verified => 4,
            Self::Retrying => 5,
            Self::Failed => 6,
        }
    }
}
/// Station counters. `Debug` keeps the historical tuple form used by replies.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct WifiStats {
    pub associations: u32,
    pub probe_successes: u32,
    pub probe_failures: u32,
    /// Debug-harness fault injection selector; zero when none is armed.
    pub fault: u8,
}
impl core::fmt::Debug for WifiStats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (
            self.associations,
            self.probe_successes,
            self.probe_failures,
            self.fault,
        )
            .fmt(f)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct NetworkDiscovery {
    pub ssid: Text<32>,
    pub rssi: i8,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_bounds_utf8_and_nul_are_enforced_without_debug_leaks() {
        assert!(WifiConfig::new(&[b'a'; 32], &[b'b'; 63], false).is_some());
        for (ssid, password) in [
            (b"".as_slice(), b"password".as_slice()),
            (&[b'a'; 33], b"password"),
            (b"ssid", b"short"),
            (b"ssid", &[b'b'; 64]),
            (b"a\0b", b"password"),
            (b"ssid", b"pass\0word"),
        ] {
            assert!(WifiConfig::new(ssid, password, false).is_none());
        }
        assert!(Text::<32>::new(&[255]).is_none());
    }

    #[test]
    fn control_error_words_are_stable_wire_text() {
        use std::vec::Vec;
        let words: Vec<_> = ControlError::ALL.iter().map(|e| e.name()).collect();
        assert_eq!(
            words,
            [
                "unconfigured",
                "scan_failed",
                "scan_timeout",
                "peer_not_found",
                "connect_failed",
                "connect_timeout",
                "connection_ended",
                "gatt_failed",
                "disconnect_failed",
                "disconnect_timeout",
                "configuration_failed",
                "link_lost",
                "runner_stopped",
            ]
        );
        let mut status = ControlStatus::new();
        assert_eq!(status.error_name(), "none");
        status.error = Some(ControlError::LinkLost);
        assert_eq!(status.error_name(), "link_lost");
    }

    #[test]
    fn wifi_state_codes_and_stats_debug_keep_reply_format() {
        use std::format;
        let codes = [
            WifiState::Unconfigured,
            WifiState::Connecting,
            WifiState::Associated,
            WifiState::AddressReady,
            WifiState::Verified,
            WifiState::Retrying,
            WifiState::Failed,
        ]
        .map(WifiState::code);
        assert_eq!(codes, [0, 1, 2, 3, 4, 5, 6]);
        let stats = WifiStats {
            associations: 3,
            probe_successes: 2,
            probe_failures: 1,
            fault: 0,
        };
        assert_eq!(format!("{:?}", stats), "(3, 2, 1, 0)");
    }
}
