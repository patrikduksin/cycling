//! A local HTTP server's exchange with the application that answers it, and
//! the soft access point phones join to reach it.
//!
//! The transport owns sockets, framing and timeouts and presents one parsed
//! request at a time. A request's body, if any, arrives in pieces the
//! application reads as they come; the transport never holds it whole. The
//! application answers by writing the whole response, head included, in
//! bounded pieces whenever there is room, then finishes it. The transport
//! closes the connection after each response.

/// Request methods the transport recognizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Delete,
    Put,
    Post,
    Options,
}

/// A parsed request head. Text that does not fit is refused by the transport
/// (path) or cut (query).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Distinguishes this request from the next one on any connection.
    pub id: u32,
    pub method: Method,
    pub path: heapless::String<96>,
    pub query: heapless::String<96>,
    /// From an `X-Session` header, a `vana` cookie or the `t` query value.
    pub session: heapless::String<32>,
    /// The client accepts gzip content encoding.
    pub gzip: bool,
    /// Body bytes the client declared, within the transport's cap; zero
    /// without a body.
    pub body: u32,
}

impl Request {
    pub fn new(method: Method) -> Self {
        Self {
            id: 0,
            method,
            path: heapless::String::new(),
            query: heapless::String::new(),
            session: heapless::String::new(),
            gzip: false,
            body: 0,
        }
    }
}

/// The application side of a local HTTP server.
pub trait Server {
    /// Accept connections. Off by default: nothing is reachable until the
    /// application opens a session.
    fn listen(&mut self, on: bool);
    /// The request waiting for an answer. It stays until `finish`, or until
    /// the peer goes away, after which it is `None` (or the next request,
    /// with another id) and writes for it are dropped.
    fn request(&self) -> Option<Request>;
    /// Move body bytes of request `id` that have arrived into `out`; returns
    /// how many. Zero while none are waiting, once the whole body was read,
    /// or once that request is gone. A body is read in order, exactly once.
    fn read(&mut self, id: u32, out: &mut [u8]) -> usize;
    /// Bytes that `write` accepts now.
    fn room(&self) -> usize;
    /// Queue response bytes for request `id`; returns how many were taken,
    /// zero once that request is gone.
    fn write(&mut self, id: u32, bytes: &[u8]) -> usize;
    /// The response to request `id` is complete.
    fn finish(&mut self, id: u32);
}

/// Soft access point settings. The password is WPA2-Personal, 8 to 63 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessPointConfig {
    pub ssid: crate::connectivity::Text<32>,
    pub password: crate::connectivity::Text<63>,
}

/// What the access point is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccessPointState {
    #[default]
    Off,
    Starting,
    /// Beaconing; `clients` stations are associated.
    Up {
        clients: u8,
    },
    Stopping,
    /// It could not start or stop; the radio stays in an uncertain state.
    Failed,
    /// This device has no access point.
    Unsupported,
}

impl AccessPointState {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Starting => "starting",
            Self::Up { .. } => "up",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Where a local server can be reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// The access point's own address while it is up.
    pub access_point: Option<[u8; 4]>,
    /// The station's address on a home network while it has one.
    pub station: Option<[u8; 4]>,
}

/// A soft access point that takes the radio from the station while it is up.
/// Requests are accepted, not completed: read `state` for the outcome.
pub trait AccessPoint {
    fn start(&mut self, config: AccessPointConfig) -> Result<(), crate::observation::Error>;
    fn stop(&mut self) -> Result<(), crate::observation::Error>;
    fn state(&self) -> AccessPointState;
    fn reach(&self) -> Reach;
    /// Hardware random words for passwords and tokens; 0 without a source.
    fn entropy(&mut self) -> u32;
}
