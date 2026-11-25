use exact_runner::{Answer, DataError, DataSource, Outcome, Request, Store, Value};
use serde_json::{json, Value as Json};

#[allow(dead_code, clippy::all)]
mod shapes {
    include!(concat!(env!("OUT_DIR"), "/shapes.rs"));
}
use shapes::{ContractValue, Link, Outcome as Done};

#[derive(Default)]
pub struct Vana;

fn idle() -> Link {
    Link { state: "idle".into(), detail: String::new(), name: String::new(), psm: 0.0 }
}

fn native(op: &str) -> Answer {
    Answer::Later(Request::native(json!({ "op": op }).to_string().into_bytes()))
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

impl DataSource for Vana {
    fn query(&mut self, source: &str, _: &[Value]) -> Result<Value, DataError> {
        match source {
            "link" => Ok(idle().to_value()),
            _ => Err(DataError::DeferredAtBake { message: format!("{source} asks the phone"), warning: false }),
        }
    }

    fn answer(&mut self, _: &mut Store, source: &str, _: &[Value]) -> Result<Answer, DataError> {
        match source {
            "link" => Ok(native("status")),
            "connect" => Ok(native("connect")),
            "disconnect" => Ok(native("disconnect")),
            _ => Err(DataError::UnknownSource(source.into())),
        }
    }

    fn parse(&mut self, _: &mut Store, source: &str, _: &[Value], outcome: Outcome) -> Result<Answer, DataError> {
        let value = match (source, reply(outcome)) {
            ("link", Ok(r)) => Link {
                state: text(&r, "state"),
                detail: text(&r, "detail"),
                name: text(&r, "name"),
                psm: r["psm"].as_f64().unwrap_or_default(),
            }
            .to_value(),
            ("link", Err(_)) => idle().to_value(),
            ("connect", Ok(r)) => {
                Done { ok: true, text: format!("Connected to {} in {} ms", text(&r, "name"), r["ms"]) }.to_value()
            }
            ("disconnect", Ok(_)) => Done { ok: true, text: "Disconnected".into() }.to_value(),
            ("connect" | "disconnect", Err(message)) => return Err(DataError::Unavailable(message)),
            _ => return Err(DataError::UnknownSource(source.into())),
        };
        Ok(Answer::Now(value))
    }

    fn app_id(&self) -> &str {
        "computer.cycling.vana"
    }

    fn grants(&self) -> &str {
        "device.bluetooth purpose.bluetooth"
    }
}
