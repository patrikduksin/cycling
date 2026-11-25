use exact_runner::{Answer, DataError, DataSource, Outcome, Request, Store, Value};
use serde_json::{json, Value as Json};

mod device;
mod wifi;

#[allow(dead_code, clippy::all)]
mod shapes {
    include!(concat!(env!("OUT_DIR"), "/shapes.rs"));
}
use shapes::{ContractValue, Fit, Link, Outcome as Done};
use std::collections::HashMap;
use wifi::{Flight, Next, Write};

#[derive(Default)]
pub struct Vana {
    flights: HashMap<String, Flight>,
    /// Whether the link is encrypted. Until it is, a join never leaves the phone.
    sealed: bool,
}

/// Where the Swift module writes a downloaded ride, under `app:/cache`.
const RIDES: &str = "rides";
/// A FIT file can be a megabyte at about 40 KB/s.
const FIT_TIMEOUT_MS: u32 = 120_000;

fn idle() -> Link {
    Link { state: "idle".into(), detail: String::new(), name: String::new(), psm: 0.0 }
}

fn native(request: Json) -> Answer {
    Answer::Later(Request::native(request.to_string().into_bytes()))
}

fn get(path: &str) -> Answer {
    native(json!({ "op": "http", "method": "GET", "path": path }))
}

/// A Wi-Fi write, `after_ms` from now.
fn send(write: &Write, after_ms: u32) -> Answer {
    native(json!({ "op": "http", "method": write.method(), "path": write.path(), "body": write.body(), "after_ms": after_ms }))
}

fn poll() -> Answer {
    native(json!({ "op": "http", "method": "GET", "path": "/api/wifi", "after_ms": wifi::POLL_MS }))
}

fn arg(args: &[Value], i: usize) -> String {
    args.get(i).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn reply(outcome: Outcome) -> Result<Json, String> {
    match outcome {
        Outcome::Response(r) if r.status == 200 => serde_json::from_slice(&r.body).map_err(|e| e.to_string()),
        Outcome::Response(r) => Err(String::from_utf8_lossy(&r.body).trim().to_string()),
        _ => Err("Bluetooth needs the iPhone app".into()),
    }
}

fn text(v: &Json, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

/// The device's raw HTTP response, from the Swift module's `{"response": raw}`.
fn raw(outcome: Outcome) -> Result<String, String> {
    let r = reply(outcome)?;
    Ok(r["response"].as_str().ok_or("no response from the bike computer")?.to_string())
}

fn http(outcome: Outcome) -> Result<String, String> {
    device::body(&raw(outcome)?).map(str::to_string)
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

impl DataSource for Vana {
    fn query(&mut self, source: &str, _: &[Value]) -> Result<Value, DataError> {
        match source {
            "link" => Ok(idle().to_value()),
            _ => Err(DataError::DeferredAtBake { message: format!("{source} asks the phone"), warning: false }),
        }
    }

    fn answer(&mut self, _: &mut Store, source: &str, args: &[Value]) -> Result<Answer, DataError> {
        match source {
            "link" => Ok(native(json!({ "op": "status" }))),
            "connect" => Ok(native(json!({ "op": "connect" }))),
            "disconnect" => Ok(native(json!({ "op": "disconnect" }))),
            "bike" => Ok(get("/api/status")),
            "rides" => Ok(get("/api/activities")),
            "today" => Ok(get("/api/workouts")),
            "wifi" => Ok(get("/api/wifi")),
            "join" if !self.sealed => Err(DataError::Unavailable(device::refusal(403))),
            "scan" | "join" | "forget" => {
                let write = match source {
                    "scan" => Write::Scan,
                    "join" => Write::Join { ssid: arg(args, 0), password: arg(args, 1) },
                    _ => Write::Forget,
                };
                let first = send(&write, 0);
                self.flights.insert(source.into(), Flight::new(write));
                Ok(first)
            }
            "fit" => Ok(native(json!({
                "op": "http", "method": "GET", "path": format!("/api/activities/{}.fit", arg(args, 0)),
                "to": format!("{RIDES}/{}", arg(args, 1)), "timeout_ms": FIT_TIMEOUT_MS,
            }))),
            _ => Err(DataError::UnknownSource(source.into())),
        }
    }

    fn parse(&mut self, _: &mut Store, source: &str, args: &[Value], outcome: Outcome) -> Result<Answer, DataError> {
        let value = match source {
            "link" => match reply(outcome) {
                Ok(r) => Link {
                    state: text(&r, "state"),
                    detail: text(&r, "detail"),
                    name: text(&r, "name"),
                    psm: r["psm"].as_f64().unwrap_or_default(),
                }
                .to_value(),
                Err(_) => idle().to_value(),
            },
            "connect" => {
                let r = reply(outcome).map_err(DataError::Unavailable)?;
                Done { ok: true, text: format!("Connected to {} in {} ms", text(&r, "name"), r["ms"]) }.to_value()
            }
            "disconnect" => {
                reply(outcome).map_err(DataError::Unavailable)?;
                Done { ok: true, text: "Disconnected".into() }.to_value()
            }
            "bike" | "rides" | "today" => {
                let body = http(outcome).map_err(DataError::Unavailable)?;
                let read = match source {
                    "bike" => device::bike(&body).map(|v| v.to_value()),
                    "rides" => device::rides(&body, offset(args)).map(|v| v.to_value()),
                    _ => device::today(&body).map(|v| v.to_value()),
                };
                read.map_err(DataError::Unavailable)?
            }
            "wifi" => http(outcome).and_then(|body| wifi::wifi(&body)).map_err(DataError::Unavailable)?.to_value(),
            "scan" | "join" | "forget" => {
                let mut flight = self
                    .flights
                    .remove(source)
                    .ok_or_else(|| DataError::Unavailable("the Wi-Fi request was lost".into()))?;
                let next = match raw(outcome) {
                    Ok(raw) => flight.advance(&raw),
                    Err(why) => Next::Failed(why),
                };
                let again = match next {
                    Next::Retry => send(&flight.write, wifi::RETRY_MS),
                    Next::Poll => poll(),
                    next => return landed(&flight.write, next).map(Answer::Now).map_err(DataError::Unavailable),
                };
                self.flights.insert(source.into(), flight);
                return Ok(again);
            }
            "fit" => {
                let r = reply(outcome).map_err(DataError::Unavailable)?;
                let head = r["response"].as_str().unwrap_or_default();
                match r["saved"].as_u64() {
                    Some(bytes) => device::saved(head, bytes),
                    None => device::body(head).and(Err("the bike computer sent no file".into())),
                }
                .map_err(DataError::Unavailable)?;
                Fit { path: format!("app:/cache/{RIDES}/{}", arg(args, 1)) }.to_value()
            }
            _ => return Err(DataError::UnknownSource(source.into())),
        };
        Ok(Answer::Now(value))
    }

    fn app_id(&self) -> &str {
        "computer.cycling.vana"
    }

    fn grants(&self) -> &str {
        "device.bluetooth purpose.bluetooth\nfs.read app:/cache/rides"
    }
}

#[cfg(test)]
mod tests;
