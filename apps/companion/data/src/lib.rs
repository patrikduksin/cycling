use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use exact_runner::{Answer, DataError, DataSource, HttpScheduling, Outcome, Request, Store, Value};
use serde_json::{json, Value as Json};

mod aead;
mod device;
mod link;
mod wifi;

#[allow(dead_code, clippy::all)]
mod shapes {
    include!(concat!(env!("OUT_DIR"), "/shapes.rs"));
}
use link::{Pairing, Session, NONCE};
use shapes::{ContractValue, Fit, Link, Outcome as Done, Pairing as Paired};
use std::collections::HashMap;
use wifi::{Flight, Next, Write};

/// The Swift module is a pipe: it finds the paired bike computer, opens the
/// channel and moves frames. This side owns the pairing, the handshake, the
/// encryption and HTTP.
#[derive(Default)]
pub struct Vana {
    flights: HashMap<String, Flight>,
    /// The encrypted session. Nothing reaches the device without one.
    session: Option<Session>,
    opening: Option<Opening>,
    /// A reply failed to authenticate: the channel closes before the source answers.
    torn: bool,
    /// What a source answers once its channel has closed.
    closing: HashMap<String, Result<Value, String>>,
}

/// How far `connect` is: the channel, the handshake, then the clock.
enum Opening {
    Channel(Pairing),
    Hello { pairing: Pairing, nonce_p: [u8; NONCE], name: String, ms: f64 },
    Clock { name: String, ms: f64 },
}

/// The secret that holds the pairing link.
const PAIRING: &str = "vana.pairing";
/// A FIT file can be a megabyte at about 40 KB/s.
const FIT_TIMEOUT_MS: u32 = 120_000;
const NOT_CONNECTED: &str = "Not connected to the bike computer";

fn link_value(state: &str, detail: String, name: String, psm: f64) -> Value {
    Link { state: state.into(), detail, name, psm }.to_value()
}

/// Requests leave in the order Rust sealed them: the device refuses a frame
/// whose counter is not the next one.
fn native(request: Json) -> Answer {
    let mut request = Request::native(request.to_string().into_bytes());
    request.http = HttpScheduling::Ordered;
    Answer::Later(request)
}

fn disconnect() -> Answer {
    native(json!({ "op": "disconnect" }))
}

fn arg(args: &[Value], i: usize) -> String {
    args.get(i).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The Swift module's JSON reply.
fn module(outcome: Outcome) -> Result<Json, String> {
    match outcome {
        Outcome::Response(r) if r.status == 200 => serde_json::from_slice(&r.body).map_err(|e| e.to_string()),
        Outcome::Response(r) => Err(String::from_utf8_lossy(&r.body).trim().to_string()),
        _ => Err("Bluetooth needs the iPhone app".into()),
    }
}

fn text(v: &Json, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn bytes(v: &Json, key: &str) -> Result<Vec<u8>, String> {
    B64.decode(v[key].as_str().unwrap_or_default()).map_err(|_| "the Bluetooth module sent bad bytes".to_string())
}

fn http_request(method: &str, path: &str, body: &str) -> Vec<u8> {
    format!("{method} {path} HTTP/1.1\r\nHost: vana\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

fn storage(op: &str, args: Json) -> Answer {
    Answer::Later(Request::storage(json!({ "version": 1, "op": op, "args": args }).to_string().into_bytes()))
}

fn stored(outcome: Outcome) -> Result<(), String> {
    match outcome {
        Outcome::Storage(bytes) => match serde_json::from_slice::<Json>(&bytes) {
            Ok(v) => v.get("error").and_then(Json::as_str).map_or(Ok(()), |e| Err(e.to_string())),
            Err(e) => Err(e.to_string()),
        },
        Outcome::Failed { message, .. } => Err(message),
        _ => Err("expected a storage result".into()),
    }
}

/// What a Wi-Fi write's source answers once its flight lands.
fn landed(write: &Write, next: Next) -> Result<Value, String> {
    Ok(match (write, next) {
        (Write::Join { ssid, .. }, next) => wifi::joined(ssid, next).to_value(),
        (_, Next::Failed(why)) => return Err(why),
        (Write::Scan, Next::Settled { error: Some(e), .. }) => return Err(format!("The scan failed ({e}).")),
        (Write::Forget, Next::Settled { error: Some(e), .. }) => return Err(format!("The bike computer kept the network ({e}).")),
        (Write::Scan, _) => Done { ok: true, text: String::new() }.to_value(),
        (Write::Forget, _) => Done { ok: true, text: "Forgot the network".into() }.to_value(),
    })
}

fn offset(args: &[Value]) -> i64 {
    args.first().and_then(Value::as_number).unwrap_or_default() as i64
}

impl Vana {
    /// An HTTP request over the session, `after_ms` from now.
    fn send(&mut self, method: &str, path: &str, body: &str, after_ms: u32, timeout_ms: u32) -> Result<Answer, DataError> {
        let session = self.session.as_mut().ok_or_else(|| DataError::Unavailable(NOT_CONNECTED.into()))?;
        let frames = session.request(&http_request(method, path, body));
        Ok(native(json!({
            "op": "exchange", "send": B64.encode(frames), "until": "end", "after_ms": after_ms, "timeout_ms": timeout_ms,
        })))
    }

    fn get(&mut self, path: &str) -> Result<Answer, DataError> {
        self.send("GET", path, "", 0, 15_000)
    }

    fn write(&mut self, write: &Write, after_ms: u32) -> Result<Answer, DataError> {
        self.send(write.method(), write.path(), &write.body(), after_ms, 15_000)
    }

    /// The device's response to an exchange, opened. The Swift module counts
    /// the frames the device sent since the channel opened, `nonce_d` first.
    fn response(&mut self, outcome: Outcome) -> Result<Vec<u8>, String> {
        let r = module(outcome)?;
        let at = r["at"].as_u64().and_then(|at| at.checked_sub(1)).ok_or("the Bluetooth module lost count of frames")?;
        let frames = bytes(&r, "frames")?;
        let session = self.session.as_mut().ok_or(NOT_CONNECTED)?;
        session.response(at, &frames).inspect_err(|_| {
            self.session = None;
            self.torn = true;
        })
    }

    fn raw(&mut self, outcome: Outcome) -> Result<String, String> {
        Ok(String::from_utf8_lossy(&self.response(outcome)?).into_owned())
    }

    fn body(&mut self, outcome: Outcome) -> Result<String, String> {
        device::body(&self.raw(outcome)?).map(str::to_string)
    }

    /// Answer `source` with `then` once the channel has closed: a broken
    /// session is not used again.
    fn close(&mut self, source: &str, then: Result<Value, String>) -> Answer {
        self.session = None;
        self.opening = None;
        self.closing.insert(source.into(), then);
        disconnect()
    }

    fn connect(&mut self, outcome: Outcome) -> Result<Answer, String> {
        match self.opening.take() {
            Some(Opening::Channel(pairing)) => {
                let r = module(outcome)?;
                let nonce_p: [u8; NONCE] = bytes(&r, "random")?.try_into().map_err(|_| "the Bluetooth module sent no nonce")?;
                let hello = link::hello(&nonce_p);
                self.opening = Some(Opening::Hello { pairing, nonce_p, name: text(&r, "name"), ms: r["ms"].as_f64().unwrap_or_default() });
                Ok(native(json!({ "op": "exchange", "send": B64.encode(hello), "until": "frame", "timeout_ms": 5000 })))
            }
            Some(Opening::Hello { pairing, nonce_p, name, ms }) => {
                let r = module(outcome)?;
                let frames = bytes(&r, "frames")?;
                let nonce_d: [u8; NONCE] = link::frames(&frames)?
                    .first()
                    .and_then(|f| (*f).try_into().ok())
                    .ok_or("the bike computer did not answer the handshake")?;
                self.session = Some(Session::open(&pairing, &nonce_p, &nonce_d));
                self.opening = Some(Opening::Clock { name, ms });
                let now = r["unix_ms"].as_f64().unwrap_or_default() as u64;
                self.send("PUT", &format!("/api/time?unix_ms={now}"), "", 0, 15_000).map_err(|e| e.to_string())
            }
            Some(Opening::Clock { name, ms }) => {
                self.body(outcome).map_err(|why| format!("The bike computer refused this iPhone ({why}). Pair it again."))?;
                Ok(Answer::Now(Done { ok: true, text: format!("Connected to {name} in {ms} ms") }.to_value()))
            }
            None => Err("the connection was lost".into()),
        }
    }
}

impl DataSource for Vana {
    fn query(&mut self, source: &str, _: &[Value]) -> Result<Value, DataError> {
        match source {
            "link" => Ok(link_value("idle", String::new(), String::new(), 0.0)),
            _ => Err(DataError::DeferredAtBake { message: format!("{source} asks the phone"), warning: false }),
        }
    }

    fn answer(&mut self, store: &mut Store, source: &str, args: &[Value]) -> Result<Answer, DataError> {
        match source {
            "link" => Ok(native(json!({ "op": "status" }))),
            "pairing" => {
                let id = store.get(PAIRING).and_then(|l| Pairing::parse(l).ok()).map(|p| p.id_hex());
                Ok(Answer::Now(Paired { paired: id.is_some(), id: id.unwrap_or_default() }.to_value()))
            }
            "pair" => {
                let pairing = Pairing::parse(&arg(args, 0)).map_err(DataError::Unavailable)?;
                store.set(PAIRING, &pairing.link()).map_err(|e| DataError::Unavailable(format!("{e:?}")))?;
                self.session = None;
                Ok(disconnect())
            }
            "unpair" => {
                store.forget(PAIRING).map_err(|e| DataError::Unavailable(format!("{e:?}")))?;
                self.session = None;
                Ok(disconnect())
            }
            "connect" => {
                let pairing = store
                    .get(PAIRING)
                    .and_then(|l| Pairing::parse(l).ok())
                    .ok_or_else(|| DataError::Unavailable("Pair this iPhone with the bike computer first".into()))?;
                let id = pairing.id_hex();
                self.session = None;
                self.opening = Some(Opening::Channel(pairing));
                Ok(native(json!({ "op": "connect", "id": id })))
            }
            "disconnect" => {
                self.session = None;
                Ok(disconnect())
            }
            "bike" => self.get("/api/status"),
            "rides" => self.get("/api/activities"),
            "today" => self.get("/api/workouts"),
            "wifi" => self.get("/api/wifi"),
            "scan" | "join" | "forget" => {
                let write = match source {
                    "scan" => Write::Scan,
                    "join" => Write::Join { ssid: arg(args, 0), password: arg(args, 1) },
                    _ => Write::Forget,
                };
                let first = self.write(&write, 0)?;
                self.flights.insert(source.into(), Flight::new(write));
                Ok(first)
            }
            "fit" => self.send("GET", &format!("/api/activities/{}.fit", arg(args, 0)), "", 0, FIT_TIMEOUT_MS),
            _ => Err(DataError::UnknownSource(source.into())),
        }
    }

    fn parse(&mut self, store: &mut Store, source: &str, args: &[Value], outcome: Outcome) -> Result<Answer, DataError> {
        if let Some(then) = self.closing.remove(source) {
            return then.map(Answer::Now).map_err(DataError::Unavailable);
        }
        let answer = self.settle(source, args, outcome);
        if !std::mem::take(&mut self.torn) {
            return answer;
        }
        Ok(self.close(
            source,
            match answer {
                Ok(Answer::Now(value)) => Ok(value),
                Err(DataError::Unavailable(why)) => Err(why),
                _ => Err("the link broke".into()),
            },
        ))
    }

    fn app_id(&self) -> &str {
        "computer.cycling.vana"
    }

    fn grants(&self) -> &str {
        "device.bluetooth purpose.bluetooth\nfs.read app:/cache\nfs.write app:/cache\nsecret.keep vana.pairing"
    }
}

impl Vana {
    fn settle(&mut self, source: &str, args: &[Value], outcome: Outcome) -> Result<Answer, DataError> {
        let value = match source {
            "link" => match module(outcome) {
                Ok(r) => {
                    let state = match text(&r, "state").as_str() {
                        "open" if self.session.is_some() && self.opening.is_none() => "open".to_string(),
                        "open" => "opening".into(),
                        state @ ("idle" | "failed") => {
                            self.session = None;
                            state.into()
                        }
                        state => state.into(),
                    };
                    link_value(&state, text(&r, "detail"), text(&r, "name"), r["psm"].as_f64().unwrap_or_default())
                }
                Err(_) => link_value("idle", String::new(), String::new(), 0.0),
            },
            "pair" | "unpair" => {
                module(outcome).map_err(DataError::Unavailable)?;
                let text = if source == "pair" { "Paired" } else { "Forgot the bike computer" };
                Done { ok: true, text: text.into() }.to_value()
            }
            "connect" => {
                let channel_open = matches!(self.opening, Some(Opening::Hello { .. } | Opening::Clock { .. }));
                return match self.connect(outcome) {
                    Ok(answer) => Ok(answer),
                    Err(why) if channel_open => Ok(self.close(source, Err(why))),
                    Err(why) => Err(DataError::Unavailable(why)),
                };
            }
            "disconnect" => {
                module(outcome).map_err(DataError::Unavailable)?;
                Done { ok: true, text: "Disconnected".into() }.to_value()
            }
            "bike" | "rides" | "today" => {
                let body = self.body(outcome).map_err(DataError::Unavailable)?;
                let read = match source {
                    "bike" => device::bike(&body).map(|v| v.to_value()),
                    "rides" => device::rides(&body, offset(args)).map(|v| v.to_value()),
                    _ => device::today(&body).map(|v| v.to_value()),
                };
                read.map_err(DataError::Unavailable)?
            }
            "wifi" => self.body(outcome).and_then(|body| wifi::wifi(&body)).map_err(DataError::Unavailable)?.to_value(),
            "scan" | "join" | "forget" => {
                let mut flight = self.flights.remove(source).ok_or_else(|| DataError::Unavailable("the Wi-Fi request was lost".into()))?;
                let next = match self.raw(outcome) {
                    Ok(raw) => flight.advance(&raw),
                    Err(why) => Next::Failed(why),
                };
                let again = match next {
                    Next::Retry => self.write(&flight.write, wifi::RETRY_MS),
                    Next::Poll => self.send("GET", "/api/wifi", "", wifi::POLL_MS, 15_000),
                    next => return landed(&flight.write, next).map(Answer::Now).map_err(DataError::Unavailable),
                };
                let again = match again {
                    Ok(again) => again,
                    Err(e) => return landed(&flight.write, Next::Failed(e.to_string())).map(Answer::Now).map_err(DataError::Unavailable),
                };
                self.flights.insert(source.into(), flight);
                return Ok(again);
            }
            "fit" => {
                let path = format!("app:/cache/{}", arg(args, 1));
                if matches!(outcome, Outcome::Storage(_) | Outcome::Failed { .. }) {
                    stored(outcome).map_err(|e| DataError::Unavailable(format!("could not save the file: {e}")))?;
                    return Ok(Answer::Now(Fit { path }.to_value()));
                }
                let response = self.response(outcome).map_err(DataError::Unavailable)?;
                let split = response.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
                let (head, body) = response.split_at(split.unwrap_or(response.len()));
                let head = String::from_utf8_lossy(head);
                match split {
                    Some(_) => device::saved(&head, body.len() as u64),
                    None => device::body(&head).and(Err("the bike computer sent no file".into())),
                }
                .map_err(DataError::Unavailable)?;
                return Ok(storage("fs.writeFile", json!({ "path": path, "bytes": body })));
            }
            _ => return Err(DataError::UnknownSource(source.into())),
        };
        Ok(Answer::Now(value))
    }
}

#[cfg(test)]
mod tests;
