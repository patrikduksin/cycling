//! The story as OTLP/JSON for OpenTelemetry tooling: one traces export
//! request and one metrics export request, a JSON object per line, as the
//! collector's `otlpjsonfile` receiver reads them. Spans and span events
//! come from the story; the metrics are the shown readings once a second,
//! the battery level, the health records and the activity windows, a task's
//! points carrying its name as `task`.
//!
//! Times are UTC when the trace holds a GPS or Wi-Fi time, else uptime
//! counted from the Unix epoch; the resource says which.
use crate::recording::json;
use crate::trace::story::Story;
use crate::trace::timeline::{Kind, Timeline, Window, field};
use firmware_services::trace::Health;
use std::fmt::Write as _;

const SHOWN: [(&str, &str); 6] = [
    ("speed", "km/h"),
    ("power", "W"),
    ("hr", "1/min"),
    ("cad", "1/min"),
    ("dist", "km"),
    ("grade", "%"),
];

const NO_RECORDED_VALUE: u32 = 1;

/// A metric's name, unit and how to read its value from one record.
type Metric<T> = (&'static str, &'static str, fn(&T) -> Option<f64>);

pub fn export(timeline: &Timeline, story: &Story) -> String {
    let base_ms = timeline.utc_at_boot_ms.unwrap_or(0);
    let nanos = |ms: u64| json(&((base_ms + ms) as u128 * 1_000_000).to_string());
    let trace_id = fnv1a(&[
        timeline.build.as_bytes(),
        &timeline.boot.to_le_bytes(),
        &base_ms.to_le_bytes(),
    ]);
    let trace_id = format!(
        "{:016x}{:016x}",
        trace_id,
        fnv1a(&[&trace_id.to_le_bytes()])
    );
    let span_id = |index: usize| {
        format!(
            "{:016x}",
            fnv1a(&[trace_id.as_bytes(), &index.to_le_bytes()])
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
        let events = story
            .events
            .iter()
            .filter(|event| event.span == index)
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
            .join(",");
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
    let health: [Metric<Health>; 14] = [
        ("heap_free", "By", |h| h.heap_free.map(f64::from)),
        ("heap_min", "By", |h| h.heap_min.map(f64::from)),
        ("psram_free", "By", |h| h.psram_free.map(f64::from)),
        ("frame_max", "us", |h| h.frame_max_us.map(f64::from)),
        ("pass_max", "us", |h| h.pass_max_us.map(f64::from)),
        ("gps.sentences", "{sentence}", |h| {
            Some(f64::from(h.gps_sentences))
        }),
        ("gps.overflows", "{error}", |h| {
            Some(f64::from(h.gps_overflows))
        }),
        ("gps.uart_errors", "{error}", |h| {
            Some(f64::from(h.gps_uart_errors))
        }),
        ("companion.errors", "{error}", |h| {
            Some(f64::from(h.companion_errors))
        }),
        ("card.write_max", "us", |h| {
            h.card_write_max_us.map(f64::from)
        }),
        ("card.recoveries", "{recovery}", |h| {
            h.card_recoveries.map(f64::from)
        }),
        ("spool.used", "By", |h| Some(f64::from(h.spool_used))),
        ("wifi.rssi", "dBm", |h| h.wifi_rssi.map(f64::from)),
        ("ble.rssi", "dBm", |h| h.ble_rssi.map(f64::from)),
    ];
    for (name, unit, read) in health {
        let points: Vec<String> = timeline
            .health
            .iter()
            .filter_map(|(at, h)| Some(number(nanos(*at), read(h)?, &[])))
            .collect();
        metrics.push(gauge(&format!("vana.health.{name}"), unit, &points));
    }
    let activity: [Metric<Window>; 6] = [
        ("wakeups", "1/s", |w| Some(per_second(w, w.activity.wakes))),
        ("ticks", "1/s", |w| Some(per_second(w, w.activity.ticks?))),
        ("awake", "1", |w| share(w, w.activity.awake_us)),
        ("radio", "1", |w| share(w, w.activity.radio_us)),
        ("idle", "1", |w| share(w, w.activity.idle_us)),
        ("backlight", "%", |w| {
            Some(f64::from(w.activity.consumers.backlight))
        }),
    ];
    for (name, unit, read) in activity {
        let points: Vec<String> = timeline
            .activity
            .iter()
            .filter_map(|w| Some(number(nanos(w.end_ms), read(w)?, &[])))
            .collect();
        metrics.push(gauge(&format!("vana.activity.{name}"), unit, &points));
    }
    let (mut wakeups, mut cpu) = (Vec::new(), Vec::new());
    for window in &timeline.activity {
        for task in &window.tasks {
            let tag = [("task", task.name.as_str())];
            wakeups.push(number(
                nanos(window.end_ms),
                per_second(window, task.polls),
                &tag,
            ));
            if let Some(busy) = share(window, task.busy_us) {
                cpu.push(number(nanos(window.end_ms), busy, &tag));
            }
        }
    }
    metrics.push(gauge("vana.activity.task.wakeups", "1/s", &wakeups));
    metrics.push(gauge("vana.activity.task.cpu", "1", &cpu));
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

/// `us` as a share of the window.
fn share(window: &Window, us: Option<u32>) -> Option<f64> {
    us.map(|us| f64::from(us) / (f64::from(window.activity.window_ms.max(1)) * 1000.0))
}

fn per_second(window: &Window, count: u32) -> f64 {
    f64::from(count) * 1000.0 / f64::from(window.activity.window_ms.max(1))
}

fn number(time: String, value: f64, attributes: &[(&str, &str)]) -> String {
    let attributes: Vec<String> = attributes.iter().map(|(k, v)| attribute(k, v)).collect();
    format!(
        r#"{{"timeUnixNano":{time},"asDouble":{value},"attributes":[{}]}}"#,
        attributes.join(",")
    )
}

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

fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
