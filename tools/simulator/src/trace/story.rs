//! A session told as OpenTelemetry-shaped spans and notable events: the
//! session, each ride and its recording and paused stretches, page visits,
//! toasts, sensor links and silences, GPS fix and companion start-up as
//! spans; boot, laps, warnings, commands and gaps as events.
use crate::trace::timeline::{
    Entry, Kind, Moment, TOAST_CLEARED, Timeline, duration, session_clock,
};
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub name: String,
    pub start: Moment,
    pub end: Moment,
    /// Index of the enclosing span; the session has none.
    pub parent: Option<usize>,
    pub attributes: Vec<(String, String)>,
    /// Still going when the trace ended.
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub at: Moment,
    pub name: String,
    pub text: String,
    /// Index of the span it happened in: the ride open then, ending
    /// included, else the session.
    pub span: usize,
    /// For a warning: what began, or which ride ended, in the same pass.
    pub alongside: Vec<String>,
}

impl Point {
    fn new(at: Moment, name: &str, text: String) -> Self {
        Self {
            at,
            name: name.into(),
            text,
            span: 0,
            alongside: Vec::new(),
        }
    }

    fn warning(&self) -> bool {
        matches!(self.name.as_str(), "log" | "storage")
    }
}

pub struct Story {
    /// The session first, then in start order.
    pub spans: Vec<Span>,
    pub events: Vec<Point>,
}

fn ride_open(state: &str) -> bool {
    matches!(state, "recording" | "paused" | "auto_paused")
}

fn state_span(entry: &Entry) -> Option<(String, Option<String>)> {
    let text = entry.text.as_str();
    let (source, name) = match entry.kind {
        Kind::Page => ("page".into(), Some(format!("page {text}"))),
        Kind::Toast => (
            "toast".into(),
            (text != TOAST_CLEARED).then(|| format!("toast {text}")),
        ),
        Kind::Ant if text.starts_with("ch") => {
            let (channel, link) = text.split_once(' ')?;
            let open = !(link == "closed" || link.starts_with("no sensor"));
            (
                format!("ant {channel}"),
                open.then(|| format!("ant {link}")),
            )
        }
        Kind::Ble if text.starts_with("link ") => (
            "ble".into(),
            (text != "link off").then(|| format!("ble {text}")),
        ),
        Kind::Gps => {
            let state = text.split(" sats").next().unwrap_or(text);
            ("gps".into(), Some(format!("gps {state}")))
        }
        Kind::Startup => ("startup".into(), Some(text.to_string())),
        _ => return None,
    };
    Some((source, name))
}

pub fn tell(timeline: &Timeline) -> Story {
    let end = timeline.moment(timeline.end_ms);
    let mut spans = vec![Span {
        name: "session".into(),
        start: timeline.moment(0),
        end,
        parent: None,
        attributes: vec![
            (
                "build".into(),
                if timeline.build.is_empty() {
                    "unknown".into()
                } else {
                    timeline.build.clone()
                },
            ),
            ("boot".into(), timeline.boot.to_string()),
            ("passes".into(), timeline.passes.to_string()),
            (
                "end".into(),
                if timeline.torn { "torn" } else { "complete" }.into(),
            ),
        ],
        open: false,
    }];
    let mut events = Vec::new();
    if let Some(first) = timeline.entries.first() {
        events.push(Point::new(
            first.at,
            "boot",
            format!("boot {}", timeline.boot),
        ));
    }
    let mut open: HashMap<String, usize> = HashMap::new();
    let mut ride: Option<usize> = None;
    let mut stretch: Option<usize> = None;
    let mut laps = 0u32;
    let close = |spans: &mut Vec<Span>, index: usize, at: Moment| {
        spans[index].end = at;
        spans[index].open = false;
    };
    for entry in timeline.entries.iter().filter(|e| !e.repeat) {
        if let Some((source, name)) = state_span(entry) {
            if let Some(index) = open.remove(&source) {
                close(&mut spans, index, entry.at);
            }
            if let Some(name) = name {
                open.insert(source, spans.len());
                spans.push(Span {
                    name,
                    start: entry.at,
                    end,
                    parent: Some(0),
                    attributes: entry
                        .cause
                        .iter()
                        .map(|cause| ("cause".into(), cause.to_string()))
                        .collect(),
                    open: true,
                });
            }
        }
        match entry.kind {
            Kind::Ride => {
                let state = entry.text.as_str();
                if let Some(index) = stretch.take() {
                    close(&mut spans, index, entry.at);
                }
                if !ride_open(state) {
                    if let Some(index) = ride.take() {
                        close(&mut spans, index, entry.at);
                        spans[index]
                            .attributes
                            .push(("laps".into(), laps.to_string()));
                        spans[index].attributes.push(("ended".into(), state.into()));
                    }
                    continue;
                }
                let parent = *ride.get_or_insert_with(|| {
                    laps = 0;
                    spans.push(Span {
                        name: "ride".into(),
                        start: entry.at,
                        end,
                        parent: Some(0),
                        attributes: Vec::new(),
                        open: true,
                    });
                    spans.len() - 1
                });
                stretch = Some(spans.len());
                spans.push(Span {
                    name: format!("ride {state}"),
                    start: entry.at,
                    end,
                    parent: Some(parent),
                    attributes: Vec::new(),
                    open: true,
                });
            }
            Kind::Toast if entry.text.starts_with("LAP ") => {
                laps += 1;
                events.push(Point::new(entry.at, "lap", entry.text.clone()));
            }
            Kind::Log | Kind::Gap | Kind::Command | Kind::Storage => {
                events.push(Point::new(entry.at, entry.kind.name(), entry.text.clone()))
            }
            _ => {}
        }
    }
    if let Some(index) = ride {
        spans[index]
            .attributes
            .push(("laps".into(), laps.to_string()));
    }
    for silence in &timeline.silences {
        spans.push(Span {
            name: format!("ant {} silent", silence.sensor),
            start: timeline.moment(silence.last_ms),
            end: silence.next_ms.map_or(end, |ms| timeline.moment(ms)),
            parent: Some(0),
            attributes: Vec::new(),
            open: silence.next_ms.is_none(),
        });
    }
    spans[1..].sort_by_key(|span| span.start.session_ms);
    reparent(&mut spans);
    attach(&mut spans, &mut events);
    Story { spans, events }
}

fn attach(spans: &mut [Span], events: &mut [Point]) {
    let mut warnings: HashMap<usize, u32> = HashMap::new();
    for event in events.iter_mut() {
        let at = event.at.session_ms;
        let within = |span: &Span| (span.start.session_ms..=span.end.session_ms).contains(&at);
        event.span = spans
            .iter()
            .rposition(|span| span.name == "ride" && within(span))
            .unwrap_or(0);
        if !event.warning() {
            continue;
        }
        event.alongside = spans[1..]
            .iter()
            .filter_map(|span| {
                if span.start.session_ms == at {
                    return Some(format!("{} began", span.name));
                }
                let ended = span.attributes.iter().find(|(key, _)| key == "ended")?;
                (span.end.session_ms == at).then(|| format!("{} ended {}", span.name, ended.1))
            })
            .collect();
        if event.span != 0 {
            *warnings.entry(event.span).or_default() += 1;
        }
    }
    for (span, count) in warnings {
        spans[span]
            .attributes
            .push(("warnings".into(), count.to_string()));
    }
}

fn reparent(spans: &mut [Span]) {
    let mut ride = None;
    for (index, span) in spans.iter_mut().enumerate() {
        if span.name == "ride" {
            ride = Some(index);
        } else if span.name.starts_with("ride ") {
            span.parent = ride;
        }
    }
}

impl fmt::Display for Span {
    /// `01:02.000 ride --:--  span  ride  -> 11:40.010 (10:38) laps=2`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}  span  {}  -> {} ({}{})",
            self.start,
            self.name,
            session_clock(self.end.session_ms),
            duration(self.end.session_ms - self.start.session_ms),
            if self.open { ", open at end" } else { "" }
        )?;
        for (key, value) in &self.attributes {
            if value.contains(' ') {
                write!(f, " {key}=\"{value}\"")?;
            } else {
                write!(f, " {key}={value}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Point {
    /// `03:55.860 ride --:--  event storage  warn sdk.facts save_failed
    /// error=... (same pass: ride ended saved, page summary began)`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  event {}  {}", self.at, self.name, self.text)?;
        if !self.alongside.is_empty() {
            write!(f, "  (same pass: {})", self.alongside.join(", "))?;
        }
        Ok(())
    }
}
