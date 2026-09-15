//! Network connection control and Embassy stack access.
#[cfg(feature = "network-stack")]
use crate::observation::{Availability, Error};

/// The concrete network handle is Embassy's Stack. Transport existence does not
/// promise DHCP/link/internet readiness; compare generations around awaited IO.
#[cfg(feature = "network-stack")]
pub trait Network {
    fn request(&mut self, _operation: crate::connectivity::WifiOperation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn control(&self) -> crate::connectivity::ControlStatus {
        crate::connectivity::ControlStatus::new()
    }
    fn discoveries(&self) -> [Option<crate::connectivity::NetworkDiscovery>; 8] {
        [None; 8]
    }

    fn online(&self) -> bool;
    fn state(&self) -> u8;
    fn stats(&self) -> (u32, u32, u32, u8);
    fn availability(&self) -> Availability;
    fn stack(&self) -> Option<embassy_net::Stack<'static>>;
    fn connection_generation(&self) -> u32;
    fn reconnect(&mut self) -> Result<(), Error>;
}
