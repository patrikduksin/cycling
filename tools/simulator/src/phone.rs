//! The phone companion's radio and transport, simulated: an access point
//! that comes up at once, phones that join when a scenario says so, and a
//! server without requests. Secrets come from a fixed sequence, so screens
//! are deterministic.
use device_api::server::{
    AccessPoint, AccessPointConfig, AccessPointState, Reach, Request, Server,
};

#[derive(Default)]
pub struct SimServer {
    pub listening: bool,
}

impl Server for SimServer {
    fn listen(&mut self, on: bool) {
        self.listening = on;
    }
    fn request(&self) -> Option<Request> {
        None
    }
    fn room(&self) -> usize {
        0
    }
    fn write(&mut self, _: u32, _: &[u8]) -> usize {
        0
    }
    fn finish(&mut self, _: u32) {}
}

#[derive(Default)]
pub struct SimAccessPoint {
    pub config: Option<AccessPointConfig>,
    /// Phones holding a lease.
    pub clients: u8,
    pub stops: u32,
    word: u32,
}

impl AccessPoint for SimAccessPoint {
    fn start(&mut self, config: AccessPointConfig) -> Result<(), device_api::observation::Error> {
        self.config = Some(config);
        Ok(())
    }
    fn stop(&mut self) -> Result<(), device_api::observation::Error> {
        self.config = None;
        self.clients = 0;
        self.stops += 1;
        Ok(())
    }
    fn state(&self) -> AccessPointState {
        match self.config {
            Some(_) => AccessPointState::Up {
                clients: self.clients,
            },
            None => AccessPointState::Off,
        }
    }
    fn reach(&self) -> Reach {
        Reach {
            access_point: self.config.map(|_| [192, 168, 4, 1]),
            station: None,
        }
    }
    fn entropy(&mut self) -> u32 {
        self.word = self
            .word
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.word
    }
}
