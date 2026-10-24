//! The phone companion's radio and transport, simulated: an access point
//! that comes up at once, phones that join when a scenario says so, and a
//! server that takes one request at a time from the scenario and hands its
//! body over in pieces, as the device's transport does. Secrets come from a
//! fixed sequence, so screens are deterministic.
use device_api::server::{
    AccessPoint, AccessPointConfig, AccessPointState, Method, Reach, Request, Server,
};

/// Body bytes the device's transport holds for the application at a time.
const INBOX: usize = 4096;

#[derive(Default)]
pub struct SimServer {
    pub listening: bool,
    exchange: Option<Exchange>,
    next_id: u32,
}

struct Exchange {
    request: Request,
    body: Vec<u8>,
    read: usize,
    response: Vec<u8>,
    finished: bool,
}

impl SimServer {
    /// A phone's request, answered over the next service steps.
    pub fn ask(&mut self, method: Method, path: &str, session: &str, body: &[u8]) {
        self.next_id += 1;
        let mut request = Request::new(method);
        request.id = self.next_id;
        request.path.push_str(path).expect("a short path");
        request.session.push_str(session).expect("a session token");
        request.body = u32::try_from(body.len()).expect("a body under 4 GB");
        self.exchange = Some(Exchange {
            request,
            body: body.to_vec(),
            read: 0,
            response: Vec::new(),
            finished: false,
        });
    }

    /// The whole response, once the device finished it.
    pub fn response(&self) -> Option<&[u8]> {
        self.exchange
            .as_ref()
            .filter(|e| e.finished)
            .map(|e| e.response.as_slice())
    }

    fn current(&mut self, id: u32) -> Option<&mut Exchange> {
        self.exchange
            .as_mut()
            .filter(|e| !e.finished && e.request.id == id)
    }
}

impl Server for SimServer {
    fn listen(&mut self, on: bool) {
        self.listening = on;
    }
    fn request(&self) -> Option<Request> {
        self.exchange
            .as_ref()
            .filter(|e| !e.finished && self.listening)
            .map(|e| e.request.clone())
    }
    fn read(&mut self, id: u32, out: &mut [u8]) -> usize {
        let Some(exchange) = self.current(id) else {
            return 0;
        };
        let n = out
            .len()
            .min(INBOX)
            .min(exchange.body.len() - exchange.read);
        out[..n].copy_from_slice(&exchange.body[exchange.read..exchange.read + n]);
        exchange.read += n;
        n
    }
    fn room(&self) -> usize {
        // The phone reads as fast as the device writes.
        if self.exchange.as_ref().is_some_and(|e| !e.finished) {
            4096
        } else {
            0
        }
    }
    fn write(&mut self, id: u32, bytes: &[u8]) -> usize {
        let Some(exchange) = self.current(id) else {
            return 0;
        };
        exchange.response.extend_from_slice(bytes);
        bytes.len()
    }
    fn finish(&mut self, id: u32) {
        if let Some(exchange) = self.current(id) {
            exchange.finished = true;
        }
    }
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
