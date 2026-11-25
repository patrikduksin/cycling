//! The bike computer's HTTP replies, as the phone shows them. The device
//! serves `apps/vana/src/app/phone.rs`'s routes over the L2CAP channel; the
//! Swift module hands back each response whole, and this module reads its
//! status, its body and the JSON in it, then words it for the view.

use crate::shapes::{Bike, Ride, Rides, Today};
use serde::Deserialize;

/// An HTTP response the device sent: its status and its body, cut to its
/// `Content-Length` when it names one.
pub struct Reply<'a> {
    pub status: u16,
    pub body: &'a str,
    length: Option<usize>,
}

pub fn response(raw: &str) -> Result<Reply<'_>, String> {
    let (head, body) = raw.split_once("\r\n\r\n").ok_or("the bike computer sent no complete reply")?;
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or("the bike computer sent no HTTP status")?;
    let length = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok())?
    });
    Ok(Reply { status, body, length })
}

/// Why the device refused, in the words the phone shows.
pub fn refusal(status: u16) -> String {
    match status {
        409 => "Bike computer is busy".into(),
        s => format!("the bike computer answered {s}"),
    }
}

fn success(raw: &str) -> Result<Reply<'_>, String> {
    let r = response(raw)?;
    if (200..300).contains(&r.status) { Ok(r) } else { Err(refusal(r.status)) }
}

/// The body of a 2xx response, or why there is none.
pub fn body(raw: &str) -> Result<&str, String> {
    let r = success(raw)?;
    match r.length {
        Some(n) => r.body.get(..n).ok_or_else(|| "the bike computer's reply was cut short".into()),
        None => Ok(r.body),
    }
}

/// A download the Swift module wrote to a file: `head` is the response
/// without its body, `bytes` what reached the file.
pub fn saved(head: &str, bytes: u64) -> Result<(), String> {
    match success(head)?.length {
        Some(n) if n as u64 != bytes => Err("the download was cut short".into()),
        _ => Ok(()),
    }
}

#[derive(Deserialize)]
struct Status {
    firmware: String,
    battery: Option<f64>,
    ftp: f64,
    lthr: f64,
    media: Vec<Medium>,
}

#[derive(Deserialize)]
struct Medium {
    free_s: f64,
}

#[derive(Deserialize)]
struct Activities {
    indexing: bool,
    activities: Vec<Activity>,
}

#[derive(Deserialize)]
struct Activity {
    id: String,
    ride: u32,
    start: Option<i64>,
    duration_s: u64,
    distance_m: Option<u64>,
    gps: bool,
    power: bool,
    heart: bool,
}

#[derive(Deserialize)]
struct Workouts {
    store: String,
    workout: Option<Workout>,
}

#[derive(Deserialize)]
struct Workout {
    name: String,
    minutes: u32,
    steps: u32,
}

pub fn json<'a, T: Deserialize<'a>>(body: &'a str) -> Result<T, String> {
    serde_json::from_str(body).map_err(|e| format!("the bike computer's reply did not read: {e}"))
}

/// `GET /api/status`.
pub fn bike(body: &str) -> Result<Bike, String> {
    let s: Status = json(body)?;
    let free = s.media.iter().map(|m| m.free_s).fold(0.0, f64::max);
    Ok(Bike {
        battery: s.battery.map_or("Unknown".into(), |pct| format!("{pct:.0}%")),
        firmware: s.firmware,
        storage: recording_left(free as u64),
        ftp: format!("{:.0} W", s.ftp),
        lthr: format!("{:.0} bpm", s.lthr),
    })
}

fn recording_left(secs: u64) -> String {
    match secs {
        0 => "Storage full".into(),
        s if s < 3600 => format!("{} min of recording left", s / 60),
        s => format!("{} h of recording left", s / 3600),
    }
}

/// `GET /api/activities`, newest first as the device lists them; dates in
/// the zone `utc_offset_min` minutes east of UTC.
pub fn rides(body: &str, utc_offset_min: i64) -> Result<Rides, String> {
    let a: Activities = json(body)?;
    Ok(Rides { indexing: a.indexing, rides: a.activities.into_iter().map(|r| ride(r, utc_offset_min)).collect() })
}

fn ride(r: Activity, utc_offset_min: i64) -> Ride {
    let local = r.start.map(|s| Civil::of(s + utc_offset_min * 60));
    let distance = r.distance_m.map(|m| format!("{:.1} km", m as f64 / 1000.0));
    let recorded: Vec<&str> =
        [(r.gps, "GPS"), (r.power, "Power"), (r.heart, "Heart rate")].into_iter().filter(|(on, _)| *on).map(|(_, name)| name).collect();
    Ride {
        title: local.as_ref().map_or(format!("Ride {}", r.ride), |c| format!("{} {} {}, {}", c.weekday(), c.day, c.month_name(), c.clock())),
        subtitle: match &distance {
            Some(km) => format!("{} · {km}", duration(r.duration_s)),
            None => duration(r.duration_s),
        },
        date: local.as_ref().map_or("Unknown".into(), |c| format!("{} {} {} {}", c.weekday(), c.day, c.month_name(), c.year)),
        start: local.as_ref().map_or("Unknown".into(), Civil::clock),
        moving: duration(r.duration_s),
        distance: distance.unwrap_or_else(|| "Not recorded".into()),
        speed: average_speed(r.distance_m, r.duration_s).unwrap_or_else(|| "Not recorded".into()),
        recorded: if recorded.is_empty() { "Time only".into() } else { recorded.join(", ") },
        file: match &local {
            Some(c) => format!("vana-{:04}-{:02}-{:02}-{}-ride{}.fit", c.year, c.month, c.day, c.clock().replace(':', ""), r.ride),
            None => format!("vana-ride{}.fit", r.ride),
        },
        id: r.id,
        power: r.power,
        heart: r.heart,
    }
}

/// Distance over moving time, in km/h.
pub fn average_speed(distance_m: Option<u64>, moving_s: u64) -> Option<String> {
    let m = distance_m?;
    (moving_s > 0).then(|| format!("{:.1} km/h", m as f64 / moving_s as f64 * 3.6))
}

/// `GET /api/workouts`.
pub fn today(body: &str) -> Result<Today, String> {
    let w: Workouts = json(body)?;
    if w.store != "ok" {
        return Err(format!("today's workout did not read ({})", w.store));
    }
    Ok(match w.workout {
        Some(w) => Today {
            planned: true,
            detail: format!("{} min · {} {}", w.minutes, w.steps, if w.steps == 1 { "step" } else { "steps" }),
            name: w.name,
        },
        None => Today { planned: false, name: "No workout today".into(), detail: String::new() },
    })
}

fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// A local time in Unix seconds, as a calendar reads it.
struct Civil {
    year: i64,
    month: u32,
    day: u32,
    weekday: usize,
    secs: i64,
}

impl Civil {
    /// Howard Hinnant's `civil_from_days`.
    fn of(local: i64) -> Civil {
        let (days, secs) = (local.div_euclid(86_400), local.rem_euclid(86_400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);
        Civil { year, month, day, weekday: days.rem_euclid(7) as usize, secs }
    }

    fn weekday(&self) -> &'static str {
        ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][self.weekday]
    }

    fn month_name(&self) -> &'static str {
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][self.month as usize - 1]
    }

    fn clock(&self) -> String {
        format!("{:02}:{:02}", self.secs / 3600, self.secs / 60 % 60)
    }
}
