//! `trace dump` and `trace undump`: a trace as JSON lines that a later
//! build re-encodes in its own format, so a format change upgrades the
//! private corpus with no old reader kept in main.
//!
//! A line holds one record as the dumping build read it. A call, and a
//! `Repeated` or `TooLarge` record, names its method, which later builds
//! may number otherwise, and a media access spells out its span, its bytes
//! and its flags. Every other record keeps its encoding. Each record also carries the value the dumping build
//! decoded from it, and the dump opens with every method's signature.
//! `undump` carries a record only when the current build decodes the same
//! value under the same signature, or when the dump is its own. The first
//! record it cannot carry ends the upgraded trace before that record's
//! pass, and is reported: a record is never defaulted.
use crate::twin::script::{Named, capabilities, declared, named};
use device_api::capability::Kind as Declared;
use device_api::journal::MethodId;
use firmware_services::trace::media::{Access, Content, PART, elided};
use firmware_services::trace::replayed::Replayed;
use firmware_services::trace::wire::{Reader, Wire, Writer};
use firmware_services::trace::{
    self, Call, Event, Header, Medium, StorageError, put_record, take_record,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;

/// This dump's own format.
pub const DUMP: u64 = 1;

const MEDIA: [Medium; 6] = [
    Medium::Configuration,
    Medium::Data,
    Medium::Crash,
    Medium::Bulk,
    Medium::Intent,
    Medium::Screens,
];
const ERRORS: [StorageError; 5] = [
    StorageError::Unsupported,
    StorageError::Unavailable,
    StorageError::Failed,
    StorageError::Invalid,
    StorageError::Range,
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (text.len().is_multiple_of(2)).then_some(())?;
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

fn signature(method: &device_api::capability::Method) -> String {
    format!("{:?} {:?} -> {}", method.kind, method.args, method.returns)
}

fn value(call: &Call<'_>) -> Value {
    match crate::trace::world::read(call) {
        Some(crate::trace::world::World::Other) | None => {
            unshown_value(call).map_or(Value::Null, Value::from)
        }
        Some(world) => format!("{world:?}").into(),
    }
}

fn unshown_value(call: &Call<'_>) -> Option<String> {
    use device_api::{ant, bulk, connectivity, display, input, observation, power};
    use device_api::{sound, storage};
    let (table, method) = declared(call.method)?;
    let mut r = Reader::new(call.body, call.now_ms);
    macro_rules! value {
        ($($ty:ty),+) => {{
            let value = ($(<$ty as Replayed>::replayed(&mut r)),+);
            (r.read() == Some(call.body.len())).then(|| format!("{value:?}"))
        }};
    }
    match (table.name, method.name) {
        ("Ant", "capabilities") => value!(ant::Capabilities),
        ("Ant", "scan") => value!(ant::ScanSnapshot),
        ("Ble", "control") => value!(connectivity::ControlStatus),
        ("Display", "availability") => value!(observation::Availability),
        ("Display", "geometry") => value!(display::Geometry),
        ("InputSource", "controls") => value!(input::Controls),
        ("OwnedFlash", "availability") => value!(storage::Region, observation::Availability),
        ("OwnedFlash", "geometry") => value!(storage::Region, storage::Geometry),
        ("Power", "battery") => value!(Option<(u8, u16, u64)>),
        ("PowerControl", "capabilities") => value!(power::Capabilities),
        ("ReadWrite", "owned_info") => value!(Result<bulk::OwnedInfo, bulk::Error>),
        ("Sound", "patterns") => value!(&[sound::Pattern]),
        _ => None,
    }
}

/// `trace` as dump lines.
pub fn dump(trace: &[u8]) -> Result<String, String> {
    let (header, events) = trace::decode(trace).map_err(|e| e.to_string())?;
    let mut terrain = [0u8; 256];
    let mut w = Writer::new(&mut terrain, 0);
    header.terrain.put(&mut w);
    let len = w.written().ok_or("the header's terrain does not fit")?;
    let terrain = &terrain[..len];
    let methods: BTreeMap<String, String> = capabilities()
        .flat_map(|table| {
            (table.methods.iter()).map(|method| {
                (
                    format!("{}::{}", table.name, method.name),
                    signature(method),
                )
            })
        })
        .collect();
    let mut lines = vec![
        json!({
            "dump": DUMP,
            "format": trace::VERSION,
            "dumped_by": env!("CYCLING_BUILD_HASH"),
            "build": hex(&header.build),
            "device": header.device,
            "boot": header.boot,
            "terrain": hex(terrain),
            "terrain_value": format!("{:?}", header.terrain),
        }),
        json!({ "methods": methods }),
    ];
    let mut pass_ms = 0;
    let mut record = vec![0u8; trace::MAX_BLOCK];
    for event in events {
        let line = match event {
            Event::Call(call) => match (declared(call.method), Access::of(&call)) {
                (Some((_, method)), Some(access)) if matches!(method.kind, Declared::Media(_)) => {
                    media(&call, &access, pass_ms)?
                }
                _ => json!({
                    "call": Named(call.method).to_string(),
                    "at": pass_ms,
                    "body": hex(call.body),
                    "value": value(&call),
                }),
            },
            Event::Repeated { method, count } => json!({
                "repeated": Named(method).to_string(),
                "count": count,
                "at": pass_ms,
            }),
            Event::TooLarge {
                capability,
                operation,
            } => json!({
                "too_large": Named(MethodId { capability, operation }).to_string(),
                "at": pass_ms,
            }),
            other => {
                let len = put_record(&other, pass_ms, &mut record).ok_or("a record too large")?;
                json!({
                    "record": hex(&record[..len]),
                    "at": pass_ms,
                    "value": format!("{other:?}"),
                })
            }
        };
        if let Event::Pass { now_ms } = event {
            pass_ms = now_ms;
        }
        lines.push(line);
    }
    let mut out = String::new();
    for line in lines {
        out += &line.to_string();
        out.push('\n');
    }
    Ok(out)
}

fn media(call: &Call<'_>, access: &Access<'_>, pass_ms: u64) -> Result<Value, String> {
    let content = match access.content {
        Content::None => Value::Null,
        Content::Digest(crc) => json!({ "digest": crc }),
        Content::Inline(_) => {
            let mut bytes = vec![0u8; access.span.count as usize * access.span.medium.unit()];
            let len = access
                .expand(&mut bytes)
                .ok_or("an inline read does not expand")?;
            json!({ "bytes": hex(&bytes[..len]) })
        }
    };
    Ok(json!({
        "media": Named(call.method).to_string(),
        "at": pass_ms,
        "medium": format!("{:?}", access.span.medium),
        "start": access.span.start,
        "count": access.span.count,
        "content": content,
        "outside": access.outside,
        "fault": access.fault.map(|fault| format!("{fault:?}")),
        "continues": access.continues,
        "each": access.each,
    }))
}

/// What `undump` made.
#[derive(Debug, Serialize)]
pub struct Undumped {
    #[serde(skip)]
    pub trace: Vec<u8>,
    pub from_format: u64,
    pub records: usize,
    /// The dump came from this build, which carries every record as it is.
    pub same_build: bool,
    /// The first record the current build cannot carry: the trace ends
    /// before its pass.
    pub stopped: Option<Stopped>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Stopped {
    pub at_ms: u64,
    pub record: String,
    pub why: String,
}

impl fmt::Display for Undumped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "format {} to {}: {} records carried",
            self.from_format,
            trace::VERSION,
            self.records
        )?;
        if let Some(stopped) = &self.stopped {
            writeln!(
                f,
                "  ends before its pass at {}: {} cannot be carried: {}",
                crate::trace::timeline::session_clock(stopped.at_ms),
                stopped.record,
                stopped.why
            )?;
        }
        Ok(())
    }
}

enum Line {
    Call { method: MethodId, body: Vec<u8> },
    Record(Vec<u8>),
}

/// Re-encode `text`, a dump of any build, in this build's format.
pub fn undump(text: &str) -> Result<Undumped, String> {
    let mut lines = text.lines().enumerate();
    let mut next = |what: &str| -> Result<Value, String> {
        let (at, line) = lines
            .next()
            .ok_or(format!("the dump ends before its {what}"))?;
        serde_json::from_str(line).map_err(|e| format!("dump line {}: {e}", at + 1))
    };
    let head = next("header")?;
    if head["dump"].as_u64() != Some(DUMP) {
        return Err(format!(
            "dump {} is not this build's ({DUMP})",
            head["dump"]
        ));
    }
    let field = |name: &str| {
        head[name]
            .as_str()
            .ok_or(format!("the header has no {name}"))
    };
    let build: [u8; 20] = unhex(field("build")?)
        .and_then(|b| b.try_into().ok())
        .ok_or("the header's build is no 20-byte hash")?;
    let terrain = unhex(field("terrain")?).ok_or("the header's terrain is no hex")?;
    let mut r = Reader::new(&terrain, 0);
    let found = Wire::take(&mut r);
    if r.read() != Some(terrain.len()) || format!("{found:?}") != field("terrain_value")? {
        return Err(format!(
            "the header's terrain reads as {found:?} here, not {}",
            field("terrain_value")?
        ));
    }
    let device = field("device")?.to_string();
    let header = Header {
        build,
        device: &device,
        boot: head["boot"]
            .as_u64()
            .and_then(|b| u32::try_from(b).ok())
            .ok_or("no boot")?,
        terrain: found,
    };
    let methods = next("methods")?;
    let signatures = methods["methods"]
        .as_object()
        .ok_or("no method signatures")?;
    let mut owned: Vec<(u64, Line)> = Vec::new();
    let same_build = head["dumped_by"].as_str() == Some(env!("CYCLING_BUILD_HASH"));
    let mut stopped = None;
    let mut last_read: LastRead = None;
    for (at, line) in text.lines().enumerate().skip(2) {
        let line: Value =
            serde_json::from_str(line).map_err(|e| format!("dump line {}: {e}", at + 1))?;
        let pass_ms = line["at"]
            .as_u64()
            .ok_or(format!("dump line {}: no at", at + 1))?;
        let carried = carry(&line, signatures, &mut last_read, same_build);
        match carried {
            Ok(carried) => owned.push((pass_ms, carried)),
            Err((record, why)) => {
                stopped = Some(Stopped {
                    at_ms: pass_ms,
                    record,
                    why,
                });
                break;
            }
        }
    }
    if stopped.is_some() {
        let opened = owned.iter().rposition(|(_, line)| {
            matches!(line, Line::Record(bytes) if matches!(take_record(bytes, 0), Some(Event::Pass { .. })))
        });
        owned.truncate(opened.unwrap_or(0));
    }
    let events: Vec<Event<'_>> = owned
        .iter()
        .map(|(pass_ms, line)| match line {
            Line::Call { method, body } => Event::Call(Call {
                method: *method,
                now_ms: *pass_ms,
                body,
            }),
            Line::Record(bytes) => take_record(bytes, *pass_ms).expect("checked as it was read"),
        })
        .collect();
    Ok(Undumped {
        trace: super::surgery::encode(&header, events.iter().copied()),
        from_format: head["format"].as_u64().unwrap_or_default(),
        records: owned.len(),
        same_build,
        stopped,
    })
}

type Uncarried = (String, String);
type LastRead = Option<(MethodId, Medium, u32, usize, Option<StorageError>)>;

fn carry(
    line: &Value,
    signatures: &serde_json::Map<String, Value>,
    last_read: &mut LastRead,
    same_build: bool,
) -> Result<Line, Uncarried> {
    let pass_ms = line["at"].as_u64().unwrap_or_default();
    if let Some(record) = line["record"].as_str() {
        *last_read = None;
        let shown = line["value"].as_str().unwrap_or_default().to_string();
        let bytes = unhex(record).ok_or((shown.clone(), "no hex".to_string()))?;
        let event = take_record(&bytes, pass_ms);
        if let Some(Event::Repeated { .. } | Event::TooLarge { .. }) = event {
            return Err((
                shown,
                "it names its method by a number this build may give another method".into(),
            ));
        }
        let read = event.map(|event| format!("{event:?}"));
        if read.as_deref() != Some(shown.as_str()) {
            let why = match read {
                Some(read) => format!("this build reads it as {read}"),
                None => "this build does not read it".into(),
            };
            return Err((shown, why));
        }
        return Ok(Line::Record(bytes));
    }
    let name = (["call", "media", "repeated", "too_large"].iter())
        .find_map(|key| line[key].as_str())
        .unwrap_or_default();
    let method =
        method_with_same_signature(name, signatures).map_err(|why| (name.to_string(), why))?;
    if line["repeated"].is_string() || line["too_large"].is_string() {
        *last_read = None;
        let event = if line["too_large"].is_string() {
            Event::TooLarge {
                capability: method.capability,
                operation: method.operation,
            }
        } else {
            let count = (line["count"].as_u64().and_then(|n| u8::try_from(n).ok()))
                .ok_or((name.to_string(), "its count is no byte".to_string()))?;
            Event::Repeated { method, count }
        };
        let mut record = [0u8; 32];
        let len = put_record(&event, pass_ms, &mut record).expect("a short record fits");
        return Ok(Line::Record(record[..len].to_vec()));
    }
    if line["media"].is_string() {
        return media_body(line, method, last_read)
            .map(|body| Line::Call { method, body })
            .map_err(|why| (name.to_string(), why));
    }
    *last_read = None;
    let body = unhex(line["body"].as_str().unwrap_or_default())
        .ok_or((name.to_string(), "no hex".into()))?;
    let call = Call {
        method,
        now_ms: pass_ms,
        body: &body,
    };
    let here = value(&call);
    let why = match (&line["value"], &here) {
        (Value::Null, Value::Null) if same_build => None,
        (Value::Null, Value::Null) => {
            Some("neither build reads its value, so the two cannot be compared".into())
        }
        (Value::Null, here) => Some(format!(
            "the dump's build did not read its value; this build reads {here}"
        )),
        (there, Value::Null) => Some(format!(
            "this build does not read the bytes the dump read as {there}"
        )),
        (there, here) if there != here => {
            Some(format!("the dump decoded {there}, this build {here}"))
        }
        _ => None,
    };
    if let Some(why) = why {
        return Err((name.to_string(), why));
    }
    Ok(Line::Call { method, body })
}

fn method_with_same_signature(
    name: &str,
    signatures: &serde_json::Map<String, Value>,
) -> Result<MethodId, String> {
    let (capability, method) = name.split_once("::").ok_or("no method name")?;
    let id = named(capability, method).ok_or("this build has no such method")?;
    let (_, declared) = declared(id).expect("a method named here is declared");
    let here = signature(declared);
    match signatures.get(name).and_then(Value::as_str) {
        Some(there) if there == here => Ok(id),
        Some(there) => Err(format!("its signature was {there}, here it is {here}")),
        None => Err("the dump lists no signature for it".into()),
    }
}

fn media_body(line: &Value, method: MethodId, last_read: &mut LastRead) -> Result<Vec<u8>, String> {
    let medium_name = line["medium"].as_str().unwrap_or_default();
    let medium = (MEDIA.into_iter())
        .find(|m| format!("{m:?}") == medium_name)
        .ok_or(format!("no medium {medium_name}"))?;
    let number = |name: &str| {
        line[name]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or(format!("no {name}"))
    };
    let span = firmware_services::trace::determined::Span {
        medium,
        start: number("start")?,
        count: number("count")?,
    };
    let fault = match line["fault"].as_str() {
        None => None,
        Some(name) => Some(
            (ERRORS.into_iter())
                .find(|e| format!("{e:?}") == name)
                .ok_or(format!("no storage error {name}"))?,
        ),
    };
    let mut runs = vec![0u8; PART * 2 + 64];
    let content = match &line["content"] {
        Value::Null => Content::None,
        content => match (content["digest"].as_u64(), content["bytes"].as_str()) {
            (Some(crc), _) => Content::Digest(u32::try_from(crc).map_err(|e| e.to_string())?),
            (None, Some(bytes)) => {
                let bytes = unhex(bytes).ok_or("no hex")?;
                runs.resize(bytes.len() + bytes.len() / 4 + 64, 0);
                let len = elided(&bytes, &mut runs).ok_or("the bytes do not elide")?;
                runs.truncate(len);
                Content::Inline(&runs)
            }
            _ => {
                return Err(format!(
                    "content {content} carries neither digest nor bytes"
                ));
            }
        },
    };
    let inline_len = match content {
        Content::Inline(_) => Some(span.count as usize * medium.unit()),
        _ => None,
    };
    let pre_format_7 = pre_format_7_continues(last_read, method, &span, fault, inline_len);
    let continues = line["continues"].as_bool().unwrap_or(pre_format_7);
    let access = Access {
        span,
        content,
        outside: line["outside"].as_bool().unwrap_or_default(),
        fault,
        continues,
        each: line["each"].as_u64().map(|each| each as u32),
    };
    let mut body = vec![0u8; runs.len() + 64];
    let mut w = Writer::new(&mut body, 0);
    access.put(&mut w);
    let len = w.written().ok_or("the record does not fit")?;
    body.truncate(len);
    Ok(body)
}

fn pre_format_7_continues(
    last_read: &mut LastRead,
    method: MethodId,
    span: &firmware_services::trace::determined::Span,
    fault: Option<StorageError>,
    inline_len: Option<usize>,
) -> bool {
    let follows_a_whole_part = last_read.is_some_and(|(m, medium, end, len, f)| {
        m == method && medium == span.medium && end == span.start && len == PART && f == fault
    }) && inline_len.is_some();
    *last_read = inline_len.map(|len| (method, span.medium, span.start + span.count, len, fault));
    follows_a_whole_part
}

#[cfg(test)]
mod tests;
