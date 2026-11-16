//! The reader: a trace's events, one by one, into entries, and what the
//! summary needs beside them.
use super::text::{bytes, duration, hex, micros, millis, percent, rate, sensor, session_clock};
use super::{
    Anchor, Cause, Entry, Kind, Moment, SILENCE_MS, Silence, StageTime, TOAST_CLEARED, TaskTime,
    Timeline, Window, field, parse_ms,
};
use crate::trace::ant_page::Pages;
use device_api::ant;
use device_api::input::Input;
use device_api::observation::Observation;
use firmware_services::trace::{self, Event, Level};
use std::fmt::{self, Write as _};

const GAP_MS: u64 = 1_000;

const TRACE_BYTES_PER_ENTRY_FLOOR: usize = 16;

/// Decode `bytes`; an error for a trace whose header does not decode.
pub fn read(bytes: &[u8]) -> Result<Timeline, String> {
    let (header, mut events) = trace::decode(bytes).map_err(|error| match error {
        trace::Error::Header => "no trace header: the file is no ride trace".to_string(),
        trace::Error::Version(_) => error.to_string(),
    })?;
    let mut reader = Reader {
        entries: Entries {
            list: Vec::with_capacity(bytes.len() / TRACE_BYTES_PER_ENTRY_FLOOR),
            last: vec![None; Entries::slot(Kind::ALL[Kind::ALL.len() - 1], u8::MAX) + 1],
            line: String::new(),
        },
        ..Reader::default()
    };
    for event in events.by_ref() {
        reader.event(&event);
    }
    reader.end_pass();
    reader.sensors.silent_at_end(reader.now);
    let build_len = header
        .build
        .iter()
        .rposition(|&b| b != 0)
        .map_or(0, |i| i + 1);
    Ok(Timeline {
        build: header.build[..build_len]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        boot: header.boot,
        passes: reader.passes,
        end_ms: reader.now,
        end: events.end().expect("events stop only once they say why"),
        entries: reader.entries.list,
        clock: reader.clock,
        utc_at_boot_ms: reader.utc_at_boot_ms,
        silences: reader.sensors.silences,
        power: reader.power,
        health: reader.health,
        frame_times: reader.frame_times,
        activity: reader.activity,
    })
}

/// The entries read so far, and each source's last entry that was no
/// repeat.
#[derive(Default)]
struct Entries {
    list: Vec<Entry>,
    last: Vec<Option<usize>>,
    /// Each entry's text is written here first, so a text is allocated once,
    /// at its own length.
    line: String,
}

impl Entries {
    /// Where a source's last entry is kept: a kind, and the ANT channel for
    /// links (0 for the rest).
    fn slot(kind: Kind, source: u8) -> usize {
        (kind as usize) << 8 | usize::from(source)
    }

    fn push_with(
        &mut self,
        at: Moment,
        kind: Kind,
        source: u8,
        write: impl FnOnce(&mut String) -> fmt::Result,
    ) -> &mut Entry {
        self.line.clear();
        write(&mut self.line).expect("a String takes any text");
        let slot = &mut self.last[Self::slot(kind, source)];
        let repeat = !kind.edge() && slot.is_some_and(|i| self.list[i].text == self.line);
        if !repeat {
            *slot = Some(self.list.len());
        }
        self.list.push(Entry {
            at,
            kind,
            text: self.line.clone(),
            repeat,
            bytes: None,
            cause: None,
            shows: None,
        });
        self.list.last_mut().expect("an entry was just pushed")
    }
}

struct Heard {
    identity: ant::Identity,
    last_ms: u64,
    name: String,
}

/// Each ANT+ sensor heard: its name, when it last sent, the silences
/// between its packets, and the page state its rates need.
#[derive(Default)]
struct Sensors {
    heard: Vec<Heard>,
    silences: Vec<Silence>,
    pages: Pages,
}

impl Sensors {
    /// Write a packet as its sensor and what it said, noting the silence it
    /// ends.
    fn packet(&mut self, now: u64, identity: ant::Identity, data: [u8; 8], line: &mut String) {
        let at = self.heard.iter().position(|heard| {
            (heard.identity.device_type, heard.identity.device_number)
                == (identity.device_type, identity.device_number)
        });
        let heard = match at {
            Some(at) => {
                let heard = &mut self.heard[at];
                heard.identity = identity;
                let last_ms = std::mem::replace(&mut heard.last_ms, now);
                if now - last_ms > SILENCE_MS {
                    self.silences.push(Silence {
                        sensor: heard.name.clone(),
                        last_ms,
                        next_ms: Some(now),
                    });
                }
                heard
            }
            None => {
                self.heard.push(Heard {
                    identity,
                    last_ms: now,
                    name: sensor(identity).to_string(),
                });
                self.heard.last_mut().expect("a sensor was just pushed")
            }
        };
        line.push_str(&heard.name);
        line.push(' ');
        self.pages.describe(line, identity, data);
    }

    fn silent_at_end(&mut self, now: u64) {
        let mut open: Vec<Silence> = self
            .heard
            .iter()
            .filter(|heard| now - heard.last_ms > SILENCE_MS)
            .map(|heard| Silence {
                sensor: heard.name.clone(),
                last_ms: heard.last_ms,
                next_ms: None,
            })
            .collect();
        self.silences.append(&mut open);
        self.silences
            .sort_by(|a, b| (a.last_ms, &a.sensor).cmp(&(b.last_ms, &b.sensor)));
    }
}

#[derive(Default)]
struct Reader {
    entries: Entries,
    sensors: Sensors,
    now: u64,
    passes: u64,
    ride_ms: Option<u64>,
    recording: bool,
    clock: Vec<Anchor>,
    utc_at_boot_ms: Option<u64>,
    /// The first entry of the pass being read.
    pass_start: usize,
    /// A page was reported before this pass.
    paged: bool,
    power: Vec<(u64, device_api::power::State)>,
    health: Vec<(u64, trace::Health)>,
    frame_times: Vec<(u64, trace::FrameTime)>,
    frame_time_awaiting_health: Option<trace::FrameTime>,
    activity: Vec<Window>,
    /// The activity window of this pass, while its tasks are read.
    window: Option<Window>,
}

impl Reader {
    fn at(&self) -> Moment {
        Moment {
            session_ms: self.now,
            ride_ms: self.ride_ms,
        }
    }

    fn learn_utc(&mut self, utc_ms: u64) {
        self.utc_at_boot_ms
            .get_or_insert(utc_ms.saturating_sub(self.now));
    }

    fn push_with(
        &mut self,
        kind: Kind,
        source: u8,
        write: impl FnOnce(&mut String) -> fmt::Result,
    ) {
        self.entries.push_with(self.at(), kind, source, write);
    }

    /// Give each page that changed in the pass just read its cause, and
    /// enter its activity window.
    fn end_pass(&mut self) {
        self.close_window();
        let pass = &mut self.entries.list[self.pass_start..];
        if pass.iter().any(|e| e.kind == Kind::Page) {
            let cause = if self.paged {
                Cause::of(pass)
            } else {
                Cause::PowerOn
            };
            self.paged = true;
            for page in pass.iter_mut().filter(|e| e.kind == Kind::Page) {
                page.cause = Some(cause.clone());
                page.shows = readings(&page.text);
            }
        }
        self.pass_start = self.entries.list.len();
    }

    /// Set the ride clock to the ride time VANA showed when they part: a
    /// storage fallback closes and reopens the recorder, which would start
    /// the clock again, while the rider's ride time carries on.
    fn ride_time_shown(&mut self, fields: &str) {
        let (Some(ride_ms), Some(shown_ms)) =
            (self.ride_ms, field(fields, "time").and_then(parse_ms))
        else {
            return;
        };
        if ride_ms / 1000 != shown_ms / 1000 {
            self.ride_ms = Some(shown_ms);
            self.clock.push(Anchor {
                session_ms: self.now,
                ride_ms: self.ride_ms,
                running: self.recording,
            });
        }
    }

    fn close_window(&mut self) {
        if let Some(window) = self.window.take() {
            self.push_with(Kind::Activity, 0, |line| {
                line.write_str(&self::window(&window))
            });
            self.activity.push(window);
        }
    }

    fn event(&mut self, event: &Event<'_>) {
        if !matches!(event, Event::FrameTime(_) | Event::Health(_)) {
            self.frame_time_awaiting_health = None;
        }
        match *event {
            Event::Pass { now_ms } => {
                self.end_pass();
                let since = now_ms.saturating_sub(self.now);
                if self.passes > 0 && since >= GAP_MS {
                    self.push_with(Kind::Gap, 0, |line| {
                        write!(
                            line,
                            "no pass for {} until {}",
                            duration(since),
                            session_clock(now_ms)
                        )
                    });
                }
                if self.recording {
                    self.ride_ms = self.ride_ms.map(|ms| ms + since);
                }
                self.now = now_ms;
                self.passes += 1;
            }
            Event::Input(edge) => {
                self.push_with(Kind::Input, 0, |line| line.write_str(&input(edge.input)))
            }
            Event::Synthetic(pressed) => self.push_with(Kind::Input, 0, |line| {
                write!(line, "synthetic {}", input(pressed))
            }),
            Event::Panel(_) => {}
            Event::Command(command) => self.push_with(Kind::Command, 0, |line| {
                line.write_str(&crate::trace::secret::shown_command(command))
            }),
            Event::Startup(startup) => self.push_with(Kind::Startup, 0, |line| {
                write!(line, "companion {}", startup.name())
            }),
            Event::AntRadio(radio) => self.push_with(Kind::Ant, 0, |line| {
                write_lower(
                    line,
                    format_args!(
                        "radio {:?}{}",
                        radio.availability,
                        if radio.scanning { " scanning" } else { "" }
                    ),
                )
            }),
            Event::AntDiscoveries(found) => self.push_with(Kind::Ant, 0, |line| {
                for (i, d) in found.iter().flatten().enumerate() {
                    write!(
                        line,
                        "{}{} rssi {}",
                        if i == 0 { "found " } else { ", " },
                        sensor(d.identity),
                        d.rssi
                    )?;
                }
                if line.is_empty() {
                    line.push_str("found nothing");
                }
                Ok(())
            }),
            Event::AntLink { channel, snapshot } => {
                self.push_with(Kind::Ant, channel, |line| ant_link(line, channel, snapshot))
            }
            Event::AntPacket(packet) => {
                let now = self.now;
                let entry = self
                    .entries
                    .push_with(self.at(), Kind::AntPacket, 0, |line| {
                        self.sensors.packet(now, packet.identity, packet.data, line);
                        Ok(())
                    });
                entry.bytes = Some(packet.data);
            }
            Event::AntRequest { operation, result } => {
                self.push_with(Kind::AntRequest, 0, |line| {
                    write!(
                        line,
                        "{} -> {}",
                        ant_operation(operation),
                        ant_result(result)
                    )
                })
            }
            Event::AntSend(send) => self.push_with(Kind::AntRequest, 0, |line| {
                write!(line, "send status {send:?}")
            }),
            Event::BleLink {
                availability,
                snapshot,
            } => self.push_with(Kind::Ble, 0, |line| {
                write!(line, "link {}", snapshot.link.name())?;
                if availability != device_api::observation::Availability::Ready {
                    let start = line.len();
                    write!(line, " ({availability:?})")?;
                    lowercase_from(line, start);
                }
                Ok(())
            }),
            Event::BlePacket(packet) => self.push_with(Kind::BlePacket, 0, |line| {
                write!(
                    line,
                    "connection {} seq {} data {}",
                    packet.connection,
                    packet.sequence,
                    hex(packet.data().unwrap_or(&[]))
                )
            }),
            Event::BleBattery(level) => self.push_with(Kind::Ble, 0, |line| {
                write!(line, "sensor battery raw {}", level.raw)
            }),
            Event::Position {
                availability: _,
                snapshot,
            } => {
                if let Some(utc) = snapshot.and_then(|s| s.gps.utc_ms) {
                    self.learn_utc(utc);
                }
                self.push_with(Kind::Gps, 0, |line| gps(line, snapshot));
            }
            Event::Physical(snapshot) => self.push_with(Kind::Sensors, 0, |line| {
                line.write_str(match snapshot.pressure {
                    Observation::Unavailable => "barometer unavailable",
                    Observation::Fresh { .. } => "barometer fresh",
                    Observation::Stale { .. } => "barometer stale",
                })
            }),
            Event::Light(light) => match light {
                Observation::Unavailable => self.push_with(Kind::Sensors, 0, |line| {
                    line.write_str("ambient light unavailable")
                }),
                Observation::Fresh { value, .. } => self.push_with(Kind::Sensors, 0, |line| {
                    write!(line, "ambient light {}", value.0)
                }),
                Observation::Stale { value, .. } => self.push_with(Kind::Sensors, 0, |line| {
                    write!(line, "ambient light {} stale", value.0)
                }),
            },
            Event::Power(snapshot) => {
                self.push_with(Kind::Battery, 0, |line| battery(line, snapshot))
            }
            Event::NetworkTime(snapshot) => {
                if let Some(unix) = snapshot.unix_seconds {
                    self.learn_utc(unix * 1000 + u64::from(snapshot.millis));
                }
                match snapshot.unix_seconds {
                    Some(unix) => self.push_with(Kind::Time, 0, |line| {
                        write_lower(
                            line,
                            format_args!("wifi utc {unix} ({:?})", snapshot.status),
                        )
                    }),
                    None => self.push_with(Kind::Time, 0, |line| {
                        write_lower(line, format_args!("wifi utc none ({:?})", snapshot.status))
                    }),
                }
            }
            Event::StorageFault(fault) => self.push_with(Kind::Storage, 0, |line| {
                write_lower(
                    line,
                    format_args!(
                        "{:?} {:?} failed at {}: {:?}",
                        fault.medium, fault.op, fault.at, fault.error
                    ),
                )
            }),
            Event::Entropy(value) => {
                self.push_with(Kind::Entropy, 0, |line| write!(line, "{value:#018x}"))
            }
            Event::Page(page) => self.push_with(Kind::Page, 0, |line| line.write_str(page)),
            Event::Toast(text) => self.push_with(Kind::Toast, 0, |line| {
                line.write_str(if text.is_empty() { TOAST_CLEARED } else { text })
            }),
            Event::Sound {
                pattern,
                cue,
                note,
                of,
            } => self.push_with(Kind::Sound, 0, |line| {
                line.write_str(&match (cue.replace('_', " "), of) {
                    (cue, _) if cue.is_empty() => format!("pattern {pattern}"),
                    (cue, 1) => format!("pattern {pattern}: {cue}"),
                    (cue, of) => format!("pattern {pattern}: {cue} ({note} of {of})"),
                })
            }),
            Event::RideState(state) => {
                let open = matches!(state, "recording" | "paused" | "auto_paused");
                self.ride_ms = match (self.ride_ms, open) {
                    (_, false) => None,
                    (None, true) => Some(0),
                    (Some(ms), true) => Some(ms),
                };
                self.recording = state == "recording";
                self.clock.push(Anchor {
                    session_ms: self.now,
                    ride_ms: self.ride_ms,
                    running: self.recording,
                });
                self.push_with(Kind::Ride, 0, |line| line.write_str(state));
            }
            Event::Mark {
                count,
                pressed_ms,
                page,
            } => self.push_with(Kind::Mark, 0, |line| {
                write!(
                    line,
                    "{count} pressed {} on {page}",
                    session_clock(pressed_ms)
                )
            }),
            Event::Shown(fields) => {
                self.ride_time_shown(fields);
                self.push_with(Kind::Shown, 0, |line| line.write_str(fields))
            }
            Event::Log {
                level,
                target,
                text,
            } => self.push_with(
                if about_storage(text) {
                    Kind::Storage
                } else {
                    Kind::Log
                },
                0,
                |line| {
                    write!(
                        line,
                        "{} {target} {text}",
                        match level {
                            Level::Error => "error",
                            Level::Warn => "warn",
                        }
                    )
                },
            ),
            Event::AccessPoint { state, reach } => self.push_with(Kind::Phone, 0, |line| {
                write_lower(line, format_args!("access point {state:?}, {reach:?}"))
            }),
            Event::AccessPointRequest { start, result } => self.push_with(Kind::Phone, 0, |line| {
                write!(
                    line,
                    "access point {} -> {}",
                    if start { "start" } else { "stop" },
                    answer(result)
                )
            }),
            Event::Server { request, room } => self.push_with(Kind::Phone, 0, |line| {
                line.write_str(&match request {
                    Some(request) => format!(
                        "server holds #{} {:?} {}{}{} body {} B, room {room} B",
                        request.id,
                        request.method,
                        request.path,
                        if request.query.is_empty() { "" } else { "?" },
                        crate::trace::secret::shown_query(request.query),
                        request.body
                    ),
                    None => format!("server holds nothing, room {room} B"),
                })
            }),
            Event::ServerListen(listen) => self.push_with(Kind::Phone, 0, |line| {
                line.write_str(if listen {
                    "server listen"
                } else {
                    "server stop"
                })
            }),
            Event::ServerFinish(id) => {
                self.push_with(Kind::Phone, 0, |line| write!(line, "server finish #{id}"))
            }
            Event::ServerRead { id, part, more } => self.push_with(Kind::Phone, 0, |line| {
                write!(
                    line,
                    "server read #{id} {} B{}",
                    part.len(),
                    if more { ", more" } else { "" }
                )
            }),
            Event::ServerWrite { id, taken } => self.push_with(Kind::Phone, 0, |line| {
                write!(line, "server write #{id} took {taken} B")
            }),
            Event::PowerControl(status) => {
                self.power.push((self.now, status.state));
                self.push_with(Kind::Power, 0, |line| {
                    write_lower(
                        line,
                        format_args!("{:?} {:?}", status.state, status.operation),
                    )
                })
            }
            Event::PowerRequest {
                operation,
                delay_ms,
                result,
            } => self.push_with(Kind::Power, 0, |line| {
                write!(
                    line,
                    "{} in {delay_ms} ms -> {}",
                    format!("{operation:?}").to_lowercase(),
                    answer(result)
                )
            }),
            Event::PoweredOff => {
                self.push_with(Kind::Power, 0, |line| line.write_str("powered off"))
            }
            Event::Backlight(availability) => self.push_with(Kind::Display, 0, |line| {
                write_lower(line, format_args!("backlight {availability:?}"))
            }),
            Event::Refused { output, attempt } => self.push_with(Kind::Display, 0, |line| {
                write_lower(line, format_args!("{output:?} refused, attempt {attempt}"))
            }),
            Event::SoundState(snapshot) => self.push_with(Kind::Buzzer, 0, |line| {
                write_lower(
                    line,
                    format_args!("{:?} {:?}", snapshot.state, snapshot.pattern),
                )
            }),
            Event::SoundRequest { play, result } => self.push_with(Kind::Buzzer, 0, |line| {
                write!(
                    line,
                    "{} -> {}",
                    match play {
                        Some(pattern) => format!("play {pattern}"),
                        None => "stop".into(),
                    },
                    answer(result)
                )
            }),
            Event::TooLarge(tag) => self.push_with(Kind::Trace, 0, |line| {
                write!(line, "a record tagged {tag} was too large and left out")
            }),
            Event::HostWrote => self.push_with(Kind::Trace, 0, |line| {
                line.write_str("a host wrote FILES or TERRAIN; the trace ends here")
            }),
            Event::Lost { blocks, bytes } => self.push_with(Kind::Trace, 0, |line| {
                write!(
                    line,
                    "{blocks} blocks, {bytes} B, lost here: the recorder had no room for them"
                )
            }),
            Event::FrameTime(frame_time) => self.frame_time_awaiting_health = Some(frame_time),
            Event::Health(health) => {
                let frame_time = self.frame_time_awaiting_health.take();
                self.push_with(Kind::Health, 0, |line| {
                    line.write_str(&self::health(&health, frame_time))
                });
                self.health.push((self.now, health));
                self.frame_times.extend(frame_time.map(|f| (self.now, f)));
            }
            Event::Activity(activity) => {
                self.close_window();
                self.window = Some(Window {
                    end_ms: self.now,
                    activity,
                    tasks: Vec::new(),
                    stages: Vec::new(),
                });
            }
            Event::Task {
                name,
                polls,
                busy_us,
            } => {
                if let Some(window) = &mut self.window {
                    window.tasks.push(TaskTime {
                        name: name.into(),
                        polls,
                        busy_us,
                    });
                }
            }
            Event::Stage {
                name,
                runs,
                busy_us,
                max_us,
            } => {
                if let Some(window) = &mut self.window {
                    window.stages.push(StageTime {
                        name: name.into(),
                        runs,
                        busy_us,
                        max_us,
                    });
                }
            }
        }
    }
}

/// Which shown readings a ride page draws, from VANA's own pages; the main
/// page's lower line as the rider left it by default.
fn readings(page: &str) -> Option<String> {
    use vana::screens::ride::{PAUSED_READINGS, RidePage, slots::Slots};
    let readings = match page {
        "ride/paused" => PAUSED_READINGS.to_vec(),
        name => match RidePage::named(name)?.readings(Slots::default()) {
            Some(readings) => readings.to_vec(),
            None => return Some("the workout step's target".into()),
        },
    };
    Some(if readings.is_empty() {
        "none".into()
    } else {
        readings.join(",")
    })
}

fn answer(result: Result<(), device_api::observation::Error>) -> String {
    match result {
        Ok(()) => "ok".into(),
        Err(error) => format!("{error:?}").to_lowercase(),
    }
}

const STORAGE_WORDS: [&str; 11] = [
    "save",
    "load",
    "forget",
    "fold",
    "clear",
    "history",
    "storage",
    "bulk",
    "media",
    "volume",
    "turns_not",
];

fn about_storage(text: &str) -> bool {
    let text = text.to_lowercase();
    STORAGE_WORDS.iter().any(|word| text.contains(word))
}

fn write_lower(line: &mut String, text: fmt::Arguments<'_>) -> fmt::Result {
    line.write_fmt(text)?;
    lowercase_from(line, 0);
    Ok(())
}

fn lowercase_from(line: &mut String, start: usize) {
    if line[start..].is_ascii() {
        line[start..].make_ascii_lowercase();
    } else {
        let lower = line[start..].to_lowercase();
        line.truncate(start);
        line.push_str(&lower);
    }
}

fn ant_link(out: &mut String, channel: u8, snapshot: Option<ant::Snapshot>) -> fmt::Result {
    match snapshot {
        None => write!(out, "ch{channel} closed"),
        Some(s) => {
            match s.selected {
                Some(peer) => write!(out, "ch{channel} {}", sensor(peer)),
                None => write!(out, "ch{channel} no sensor"),
            }?;
            write!(
                out,
                " {}{}{}",
                s.link.name(),
                if s.stale { " stale" } else { "" },
                if s.scanning { " scanning" } else { "" }
            )
        }
    }
}

fn ant_operation(operation: ant::AntOperation) -> String {
    match operation {
        ant::AntOperation::Scan(ms) => format!("scan {ms} ms"),
        ant::AntOperation::StopScan => "stop scan".into(),
        ant::AntOperation::Connect(peer) => format!("connect {}", sensor(peer)),
        ant::AntOperation::Disconnect(channel) => format!("disconnect ch{channel}"),
        ant::AntOperation::Send { target, data } => {
            format!("send {target:?} data {}", hex(&data))
        }
    }
}

fn ant_result(result: Result<ant::Admission, ant::Error>) -> String {
    match result {
        Ok(admission) => format!("{admission:?}"),
        Err(error) => format!("refused {error:?}"),
    }
    .to_lowercase()
}

fn input(input: Input) -> String {
    match input {
        Input::Button { button, code } => format!("button {button:?} code {code}"),
        Input::Touch(point) => format!("touch {},{}", point.x, point.y),
        Input::Release => "release".into(),
        Input::Cancel => "cancel".into(),
    }
}

fn gps(out: &mut String, snapshot: Option<device_api::positioning::Snapshot>) -> fmt::Result {
    let Some(snapshot) = snapshot else {
        return out.write_str("no receiver");
    };
    let fix = snapshot.gps;
    let state = match fix.state {
        device_api::gps::FixState::NoData => "no data",
        device_api::gps::FixState::NoFix => "no fix",
        device_api::gps::FixState::Fresh => "fix",
        device_api::gps::FixState::Stale => "fix stale",
    };
    out.push_str(state);
    if let Some(satellites) = fix.satellites {
        write!(out, " sats {satellites}")?;
    }
    Ok(())
}

fn battery(out: &mut String, snapshot: Option<device_api::input::InputSnapshot>) -> fmt::Result {
    let Some(snapshot) = snapshot else {
        return out.write_str("companion silent");
    };
    match snapshot.battery {
        Observation::Fresh {
            value: (percent, _),
            ..
        } => write!(out, "{percent}%"),
        Observation::Stale {
            value: (percent, _),
            ..
        } => write!(out, "{percent}% stale"),
        Observation::Unavailable => out.write_str("level unknown"),
    }?;
    out.write_str(match snapshot.power {
        Observation::Fresh { value: 0, .. } | Observation::Stale { value: 0, .. } => " charging",
        Observation::Unavailable => "",
        _ => " on battery",
    })
}

/// A health record as one timeline line.
fn health(h: &trace::Health, frame_time: Option<trace::FrameTime>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if h.simulated {
        parts.push("simulated".into());
    }
    if let Some(free) = h.heap_free {
        parts.push(format!("heap {}", bytes(u64::from(free))));
    }
    if let Some(least) = h.heap_min {
        parts.push(format!("least {}", bytes(u64::from(least))));
    }
    if let Some(free) = h.psram_free {
        parts.push(format!("psram {}", bytes(u64::from(free))));
    }
    match (frame_time, h.frame_max_us) {
        (Some(frame), Some(max_us)) => parts.push(format!(
            "frame mean {:.2} ms, max {}",
            f64::from(frame.mean_us) / 1000.0,
            millis(max_us)
        )),
        (_, Some(max_us)) => parts.push(format!("frame {}", millis(max_us))),
        _ => {}
    }
    if let Some(us) = h.pass_max_us {
        parts.push(format!("pass {}", millis(us)));
    }
    parts.push(format!(
        "gps {} sentences, {} overflows, {} uart errors",
        h.gps_sentences, h.gps_overflows, h.gps_uart_errors
    ));
    parts.push(format!("companion {} errors", h.companion_errors));
    if let Some(us) = h.card_write_max_us {
        parts.push(format!("card write {}", millis(us)));
    }
    if let Some(recoveries) = h.card_recoveries {
        parts.push(format!("card {recoveries} recoveries"));
    }
    if h.spool_size > 0 {
        parts.push(format!(
            "spool {} of {}",
            bytes(u64::from(h.spool_used)),
            bytes(u64::from(h.spool_size))
        ));
    }
    if let Some(state) = h.wifi {
        parts.push(format!("wifi {}", wifi(state)));
    }
    if let Some(rssi) = h.wifi_rssi {
        parts.push(format!("wifi {rssi} dBm"));
    }
    if let Some(rssi) = h.ble_rssi {
        parts.push(format!("ble sensor {rssi} dBm"));
    }
    parts.join(", ")
}

/// An activity window as one timeline line.
fn window(window: &Window) -> String {
    let a = &window.activity;
    let span_ms = u64::from(a.window_ms).max(1);
    let share = |us: u32| percent(f64::from(us) / (span_ms * 1000) as f64);
    let per_s = |count: u32| rate(f64::from(count) * 1000.0 / span_ms as f64);
    let mut text = format!(
        "{}{}: {} wakeups",
        if a.simulated { "simulated " } else { "" },
        duration(span_ms),
        per_s(a.wakes),
    );
    if let Some(ticks) = a.ticks {
        let _ = write!(text, ", {} ticks", per_s(ticks));
    }
    if let Some(switches) = a.switches {
        let _ = write!(text, ", {} switches", per_s(switches));
    }
    let cpu: Vec<String> = [
        ("awake", a.awake_us),
        ("main", a.main_us),
        ("radio", a.radio_us),
        ("idle", a.idle_us),
        ("counting", a.hooks_us),
    ]
    .into_iter()
    .filter_map(|(name, us)| Some(format!("{name} {}", share(us?))))
    .collect();
    if !cpu.is_empty() {
        let _ = write!(text, ", {}", cpu.join(", "));
    }
    let tasks: Vec<String> = window
        .tasks
        .iter()
        .map(|task| match task.busy_us {
            Some(busy) => format!("{} {} {}", task.name, per_s(task.polls), share(busy)),
            None => format!("{} {}", task.name, per_s(task.polls)),
        })
        .collect();
    if !tasks.is_empty() {
        let _ = write!(text, "; {}", tasks.join(", "));
    }
    let stages: Vec<String> = window
        .stages
        .iter()
        .map(|stage| {
            format!(
                "{} {} ({} a pass, max {})",
                stage.name,
                share(stage.busy_us),
                micros(u64::from(stage.busy_us) / u64::from(stage.runs.max(1))),
                micros(u64::from(stage.max_us))
            )
        })
        .collect();
    if !stages.is_empty() {
        let _ = write!(text, "; pass stages {}", stages.join(", "));
    }
    let c = &a.consumers;
    let _ = write!(
        text,
        "; backlight {}%, {} frames, gnss {}{}, ap {}, ble {}, ant {}{}{}",
        c.backlight,
        c.frames,
        match c.gnss {
            trace::Gnss::Off => "off",
            trace::Gnss::Receiving => "on",
            trace::Gnss::Paused => "paused",
            trace::Gnss::Failed => "failed",
        },
        c.wifi
            .map(|state| format!(", wifi {}", wifi(state)))
            .unwrap_or_default(),
        match c.access_point {
            device_api::server::AccessPointState::Off
            | device_api::server::AccessPointState::Unsupported => "off".to_string(),
            device_api::server::AccessPointState::Up { clients } => format!("up {clients}"),
            other => format!("{other:?}").to_lowercase(),
        },
        c.ble.name(),
        format!("{:?}", c.ant).to_lowercase(),
        if c.ant_scanning { " scanning" } else { "" },
        if c.ant_channels > 0 {
            format!(" {} linked", c.ant_channels)
        } else {
            String::new()
        }
    );
    if let Some(mhz) = c.cpu_mhz {
        let _ = write!(text, ", cpu {mhz} MHz");
    }
    if c.slept_ms > 0 {
        let _ = write!(text, ", slept {}", duration(u64::from(c.slept_ms)));
    }
    text
}

fn wifi(state: device_api::connectivity::WifiState) -> String {
    format!("{state:?}").to_lowercase()
}
