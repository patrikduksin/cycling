//! The bike computer's Wi-Fi, as the phone shows it and drives it. The
//! device serves `apps/vana/src/app/phone/wifi.rs`'s routes: `GET /api/wifi`
//! reads the station, and a scan, a join or a forget answers `202
//! {"seq":n}` at once and finishes later, so a [`Flight`] polls the read
//! until the station's operation `n` settles.

use crate::device::{json, refusal, response};
use crate::shapes::{Joined, Network, Wifi};
use serde::Deserialize;

#[derive(Deserialize)]
struct Station {
    configured: Option<String>,
    state: String,
    rssi: Option<i32>,
    op: Op,
    networks: Vec<Seen>,
}

#[derive(Deserialize)]
struct Op {
    seq: u32,
    pending: bool,
    error: Option<String>,
}

#[derive(Deserialize)]
struct Seen {
    ssid: String,
    rssi: i32,
    secure: bool,
}

/// `GET /api/wifi`.
pub fn wifi(body: &str) -> Result<Wifi, String> {
    let s: Station = json(body)?;
    let connected = s.state == "connected" || s.state == "no_internet";
    let ssid = s.configured.unwrap_or_default();
    let strength = s.rssi.filter(|_| connected).map_or(0, bars);
    Ok(Wifi {
        configured: !ssid.is_empty(),
        state: state_label(&s.state).into(),
        ink: state_ink(&s.state).into(),
        bars: f64::from(strength),
        signal: signal(strength).into(),
        networks: s
            .networks
            .into_iter()
            .map(|n| Network {
                joined: s.state == "connected" && n.ssid == ssid,
                bars: f64::from(bars(n.rssi)),
                signal: signal(bars(n.rssi)).into(),
                secure: n.secure,
                ssid: n.ssid,
            })
            .collect(),
        ssid,
    })
}

pub fn state_label(state: &str) -> &'static str {
    match state {
        "connected" => "Connected",
        "joining" => "Joining…",
        "no_internet" => "No Internet",
        "off" => "Not Set Up",
        "failed" => "Failed",
        _ => "Not Connected",
    }
}

fn state_ink(state: &str) -> &'static str {
    match state {
        "connected" => "vana.green",
        "joining" | "no_internet" => "vana.amber",
        "failed" => "vana.red",
        _ => "-exact-secondary-label",
    }
}

/// Signal bars, 1 to 3, as iOS's Wi-Fi list draws them.
pub fn bars(rssi: i32) -> u8 {
    match rssi {
        r if r >= -60 => 3,
        r if r >= -70 => 2,
        _ => 1,
    }
}

fn signal(bars: u8) -> &'static str {
    match bars {
        3 => "Strong signal",
        2 => "Fair signal",
        1 => "Weak signal",
        _ => "",
    }
}

/// A write the device takes later: what to send, and what it is for.
#[derive(Clone, Debug, PartialEq)]
pub enum Write {
    Scan,
    Join { ssid: String, password: String },
    Forget,
}

impl Write {
    pub fn method(&self) -> &'static str {
        match self {
            Write::Scan => "POST",
            Write::Join { .. } => "PUT",
            Write::Forget => "DELETE",
        }
    }

    pub fn path(&self) -> &'static str {
        match self {
            Write::Scan => "/api/wifi/scan",
            _ => "/api/wifi",
        }
    }

    pub fn body(&self) -> String {
        match self {
            Write::Join { ssid, password } => serde_json::json!({ "ssid": ssid, "password": password }).to_string(),
            _ => String::new(),
        }
    }

    fn polls(&self) -> u32 {
        match self {
            Write::Join { .. } => JOIN_POLLS,
            _ => 15,
        }
    }
}

pub const JOIN_POLLS: u32 = 30;
pub const POLL_MS: u32 = 1000;
pub const RETRY_MS: u32 = 1500;

/// One write and the polls that follow it.
#[derive(Debug, PartialEq)]
pub struct Flight {
    pub write: Write,
    seq: Option<u32>,
    retried: bool,
    polls: u32,
}

/// What a flight does with the device's latest reply.
#[derive(Debug, PartialEq)]
pub enum Next {
    /// Send the write again, after `RETRY_MS`: the device was busy.
    Retry,
    /// Read the station again, after `POLL_MS`.
    Poll,
    /// The operation is over: the station's state and error then.
    Settled { state: String, error: Option<String> },
    Failed(String),
}

impl Flight {
    pub fn new(write: Write) -> Flight {
        Flight { write, seq: None, retried: false, polls: 0 }
    }

    /// The raw HTTP response to the last request the flight sent.
    pub fn advance(&mut self, raw: &str) -> Next {
        match self.step(raw) {
            Ok(next) => next,
            Err(why) => Next::Failed(why),
        }
    }

    fn step(&mut self, raw: &str) -> Result<Next, String> {
        let r = response(raw)?;
        let Some(seq) = self.seq else {
            return match r.status {
                202 => {
                    #[derive(Deserialize)]
                    struct Accepted {
                        seq: u32,
                    }
                    self.seq = Some(json::<Accepted>(r.body)?.seq);
                    Ok(Next::Poll)
                }
                409 if !self.retried => {
                    self.retried = true;
                    Ok(Next::Retry)
                }
                400 => Err(bad_request(r.body)),
                s => Err(refusal(s)),
            };
        };
        if !(200..300).contains(&r.status) {
            return Err(refusal(r.status));
        }
        let s: Station = json(r.body)?;
        if s.op.seq >= seq && !s.op.pending {
            return Ok(Next::Settled { state: s.state, error: s.op.error });
        }
        self.polls += 1;
        if self.polls > self.write.polls() {
            return Err(match &self.write {
                Write::Join { ssid, .. } => format!("The bike computer did not join “{ssid}” within 30 seconds."),
                Write::Scan => "The scan did not finish.".into(),
                Write::Forget => "The bike computer did not forget the network.".into(),
            });
        }
        Ok(Next::Poll)
    }
}

fn bad_request(body: &str) -> String {
    #[derive(Deserialize)]
    struct Refused {
        error: String,
    }
    match json::<Refused>(body).map(|r| r.error).as_deref() {
        Ok("password") => "The password must be 8 to 63 characters.".into(),
        Ok("ssid") => "The bike computer cannot join a network with that name.".into(),
        _ => "The bike computer could not read the request.".into(),
    }
}

/// How a join ended, for the view.
pub fn joined(ssid: &str, next: Next) -> Joined {
    let (outcome, text) = match next {
        Next::Settled { error: None, state } if state == "connected" => ("joined", String::new()),
        Next::Settled { error: None, state } if state == "no_internet" => ("no_internet", "Joined, no internet".into()),
        Next::Settled { error: Some(e), .. } if e == "wrong_password" => ("wrong_password", String::new()),
        Next::Settled { error: Some(e), .. } if e == "network_not_found" => {
            ("failed", format!("The bike computer cannot find “{ssid}”."))
        }
        Next::Settled { error: Some(e), .. } => ("failed", format!("The bike computer could not join “{ssid}” ({e}).")),
        Next::Settled { state, .. } => ("failed", format!("The bike computer could not join “{ssid}” ({state}).")),
        Next::Failed(why) => ("failed", why),
        Next::Retry | Next::Poll => ("failed", "The join did not finish.".into()),
    };
    Joined { outcome: outcome.into(), ssid: ssid.into(), text }
}
