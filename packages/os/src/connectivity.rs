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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlStatus {
    pub sequence: u32,
    pub operation: Operation,
    pub error: &'static str,
}
impl ControlStatus {
    pub const fn new() -> Self {
        Self {
            sequence: 0,
            operation: Operation::Idle,
            error: "none",
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
}

/// Connection intent survives unexpected loss; explicit configuration/removal stops retries.
#[derive(Default)]
pub struct Reconnect {
    requested: bool,
    failures: u8,
}
impl Reconnect {
    pub fn connect(&mut self) {
        self.requested = true;
        self.failures = 0;
    }
    pub fn disconnect(&mut self) {
        self.requested = false;
        self.failures = 0;
    }
    pub fn requested(&self) -> bool {
        self.requested
    }
    pub fn recovered(&mut self) {
        self.failures = 0;
    }
    pub fn delay(&mut self) -> u64 {
        let delay = crate::network::retry_delay_secs(self.failures);
        self.failures = self.failures.saturating_add(1);
        delay
    }
}
#[cfg(test)]
mod recovery_tests {
    #[test]
    fn intent_survives_loss_with_capped_backoff_and_explicit_stop_cancels_it() {
        let mut retry = super::Reconnect::default();
        assert!(!retry.requested());
        retry.connect();
        assert!(retry.requested());
        let first = retry.delay();
        assert!(first > 0);
        for _ in 0..300 {
            assert!(retry.delay() <= 60);
            assert!(retry.requested());
        }
        retry.recovered();
        assert_eq!(retry.delay(), first);
        retry.disconnect();
        assert!(!retry.requested());
        retry.connect();
        assert_eq!(retry.delay(), first);
    }
}
