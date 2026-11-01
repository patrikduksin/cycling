//! The story as OTLP/JSON for OpenTelemetry tooling: one traces export
//! request and one metrics export request, a JSON object per line, as the
//! collector's `otlpjsonfile` receiver reads them. Spans and span events
//! come from the story; the metrics are the shown readings once a second
//! and the battery level.
//!
//! Times are UTC when the trace holds a GPS or Wi-Fi time, else uptime
//! counted from the Unix epoch; the resource says which.
use crate::recording::json;
use crate::trace::story::Story;
use crate::trace::timeline::{Kind, Timeline, field};
use std::fmt::Write as _;

/// Fields of the shown line exported as gauges, with their units.
const SHOWN: [(&str, &str); 6] = [
    ("speed", "km/h"),
    ("power", "W"),
    ("hr", "1/min"),
    ("cad", "1/min"),
    ("dist", "km"),
    ("grade", "%"),
];

/// A data point with no value, as a reading the screen showed as dashes.
const NO_RECORDED_VALUE: u32 = 1;

pub fn export(timeline: &Timeline, story: &Story) -> String {
    let base_ms = timeline.utc_at_boot_ms.unwrap_or(0);
    let nanos = |ms: u64| json(&((base_ms + ms) as u128 * 1_000_000).to_string());
    let trace_id = hash(&[
        timeline.build.as_bytes(),
        &timeline.boot.to_le_bytes(),
        &base_ms.to_le_bytes(),
    ]);
    let trace_id = format!("{:016x}{:016x}", trace_id, hash(&[&trace_id.to_le_bytes()]));
    let span_id = |index: usize| {
        format!(
            "{:016x}",
            hash(&[trace_id.as_bytes(), &index.to_le_bytes()])
        )
    };
    let resource = format!(
        r#"{{"attributes":[{},{},{},{},{}]}}"#,
        attribute("service.name", "vana"),
        attribute(
            "service.version",
            if timeline.build.is_empty() {
                "unknown"
            } else {
                &timeline.build
            }
        ),
        attribute("vana.boot", &timeline.boot.to_string()),
        attribute("vana.trace.version", &timeline.version.to_string()),
        attribute(
            "vana.clock",
            if timeline.utc_at_boot_ms.is_some() {
                "utc"
            } else {
                "uptime"
            }
        ),
    );
    let scope = r#"{"name":"cycling.trace","version":"1"}"#;

    let mut spans = Vec::new();
    for (index, span) in story.spans.iter().enumerate() {
        let mut attributes = vec![attribute("vana.start", &span.start.to_string())];
        attributes.extend(span.attributes.iter().map(|(k, v)| attribute(k, v)));
        if span.open {
            attributes.push(attribute("vana.open_at_end", "true"));
        }
        let events = if index == 0 {
            story
                .events
                .iter()
                .map(|event| {
                    format!(
                        r#"{{"timeUnixNano":{},"name":{},"attributes":[{},{}]}}"#,
                        nanos(event.at.session_ms),
                        json(&event.name),
                        attribute("text", &event.text),
                        attribute("vana.at", &event.at.to_string()),
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        } else {
            String::new()
        };
        let parent = span
            .parent
            .map(|parent| format!(r#""parentSpanId":{},"#, json(&span_id(parent))))
            .unwrap_or_default();
        spans.push(format!(
            r#"{{"traceId":{},"spanId":{},{parent}"name":{},"kind":1,"startTimeUnixNano":{},"endTimeUnixNano":{},"attributes":[{}],"events":[{events}],"status":{{}}}}"#,
            json(&trace_id),
            json(&span_id(index)),
            json(&span.name),
            nanos(span.start.session_ms),
            nanos(span.end.session_ms),
            attributes.join(","),
        ));
    }
    let mut out = format!(
        r#"{{"resourceSpans":[{{"resource":{resource},"scopeSpans":[{{"scope":{scope},"spans":[{}]}}]}}]}}"#,
        spans.join(",")
    );
    out.push('\n');

    let mut metrics = Vec::new();
    for (name, unit) in SHOWN {
        let points: Vec<String> = timeline
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Shown)
            .filter_map(|e| Some((e.at.session_ms, field(&e.text, name)?)))
            .map(|(at, value)| point(nanos(at), value.trim_end_matches('%')))
            .collect();
        metrics.push(gauge(&format!("vana.shown.{name}"), unit, &points));
    }
    let battery: Vec<String> = timeline
        .entries
        .iter()
        .filter(|e| e.kind == Kind::Battery && !e.repeat)
        .filter_map(|e| Some((e.at.session_ms, e.text.split_once('%')?.0)))
        .map(|(at, value)| point(nanos(at), value))
        .collect();
    metrics.push(gauge("vana.battery", "%", &battery));
    metrics.retain(|m| !m.is_empty());
    let _ = writeln!(
        out,
        r#"{{"resourceMetrics":[{{"resource":{resource},"scopeMetrics":[{{"scope":{scope},"metrics":[{}]}}]}}]}}"#,
        metrics.join(",")
    );
    out
}

fn attribute(key: &str, value: &str) -> String {
    format!(
        r#"{{"key":{},"value":{{"stringValue":{}}}}}"#,
        json(key),
        json(value)
    )
}

/// A gauge point; a value that is no number, such as `--`, is a point
/// with no recorded value.
fn point(time: String, value: &str) -> String {
    match value.parse::<f64>() {
        Ok(number) if number.is_finite() => {
            format!(r#"{{"timeUnixNano":{time},"asDouble":{number}}}"#)
        }
        _ => format!(r#"{{"timeUnixNano":{time},"flags":{NO_RECORDED_VALUE}}}"#),
    }
}

fn gauge(name: &str, unit: &str, points: &[String]) -> String {
    if points.is_empty() {
        return String::new();
    }
    format!(
        r#"{{"name":{},"unit":{},"gauge":{{"dataPoints":[{}]}}}}"#,
        json(name),
        json(unit),
        points.join(",")
    )
}

/// FNV-1a over the parts: stable ids from what names the session.
fn hash(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
