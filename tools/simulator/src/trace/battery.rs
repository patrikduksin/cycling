//! What a trace says about the battery: where the CPU time went in each
//! state of the device, which tasks keep waking or burning CPU while the
//! screen is dark, and how the device's health moved over the session.
//! It reads the activity windows and health records the device writes
//! every few seconds; a trace from before them has none. A trace cannot
//! measure current: these say what kept the CPU and radios busy, and a USB
//! power meter on the bench says what that cost.
use crate::trace::timeline::{
    Timeline, Window, bytes, duration, millis, percent, rate, session_clock,
};
use device_api::power::State as Power;
use firmware_services::trace::Health;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Wakeups a second, or a share of the CPU, that a task keeps while the
/// screen is dark before the summary names it: a dark device should wake
/// for a press, a packet or a timer that matters, not for a poll loop.
const DARK_WAKES_PER_S: f64 = 10.0;
const DARK_CPU: f64 = 0.01;
/// A main-loop pass this slow is a stall.
const STALL_US: u32 = 100_000;
/// A heap that the session's trend line drops by this much is falling.
const FALLING_BYTES: f64 = 1024.0;

/// What the device was doing as a window ended. A dark screen outside
/// charging standby is `ScreenOff` whatever the ride does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    HomeIdle,
    Riding,
    Paused,
    ScreenOff,
    ChargingStandby,
}

impl State {
    const fn name(self) -> &'static str {
        match self {
            Self::HomeIdle => "home idle",
            Self::Riding => "riding",
            Self::Paused => "paused",
            Self::ScreenOff => "screen off",
            Self::ChargingStandby => "charging standby",
        }
    }
}

/// The state each window ended in, in window order.
fn states(timeline: &Timeline) -> Vec<State> {
    timeline
        .activity
        .iter()
        .map(|window| {
            let at = window.end_ms;
            let power = timeline.power.partition_point(|(ms, _)| *ms <= at);
            let charging = power
                .checked_sub(1)
                .is_some_and(|i| timeline.power[i].1 == Power::Charging);
            let ride = timeline
                .clock
                .partition_point(|anchor| anchor.session_ms <= at)
                .checked_sub(1)
                .map(|i| timeline.clock[i])
                .filter(|anchor| anchor.ride_ms.is_some());
            match (charging, window.activity.consumers.backlight, ride) {
                (true, ..) => State::ChargingStandby,
                (false, 0, _) => State::ScreenOff,
                (false, _, Some(anchor)) if anchor.running => State::Riding,
                (false, _, Some(_)) => State::Paused,
                (false, _, None) => State::HomeIdle,
            }
        })
        .collect()
}

/// Windows added up: their time, wakeups, and what only a device measures
/// while every window has it (a simulator's have none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Totals {
    ms: u64,
    wakes: u64,
    cpu: Option<Cpu>,
    tasks: BTreeMap<String, TaskTotal>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cpu {
    awake: u64,
    radio: u64,
    idle: u64,
    hooks: u64,
    ticks: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TaskTotal {
    polls: u64,
    busy: Option<u64>,
}

impl Totals {
    fn new(cpu: bool) -> Self {
        Self {
            cpu: cpu.then(Cpu::default),
            ..Self::default()
        }
    }

    fn add(&mut self, window: &Window) {
        let a = &window.activity;
        self.ms += u64::from(a.window_ms);
        self.wakes += u64::from(a.wakes);
        self.cpu = self.cpu.and_then(|cpu| {
            Some(Cpu {
                awake: cpu.awake + u64::from(a.awake_us?),
                radio: cpu.radio + u64::from(a.radio_us?),
                idle: cpu.idle + u64::from(a.idle_us?),
                hooks: cpu.hooks + u64::from(a.hooks_us.unwrap_or(0)),
                ticks: cpu.ticks + u64::from(a.ticks.unwrap_or(0)),
            })
        });
        let cpu = self.cpu.is_some();
        for task in &window.tasks {
            let total = self.tasks.entry(task.name.clone()).or_insert(TaskTotal {
                polls: 0,
                busy: cpu.then_some(0),
            });
            total.polls += u64::from(task.polls);
            total.busy = total
                .busy
                .and_then(|busy| Some(busy + u64::from(task.busy_us?)));
        }
    }

    fn per_second(&self, count: u64) -> f64 {
        count as f64 * 1000.0 / self.ms.max(1) as f64
    }

    fn share(&self, us: u64) -> f64 {
        us as f64 / (self.ms.max(1) * 1000) as f64
    }

    /// `200/s 3.0%`, or `200/s` without CPU times.
    fn task(&self, task: TaskTotal) -> String {
        let wakes = rate(self.per_second(task.polls));
        match task.busy {
            Some(busy) => format!("{wakes} {}", percent(self.share(busy))),
            None => wakes,
        }
    }
}

/// The summary's activity, flags and health lines; nothing for a trace
/// without activity or health records.
pub fn summary(timeline: &Timeline) -> String {
    let mut out = String::new();
    if !timeline.activity.is_empty() {
        activity(timeline, &mut out);
    }
    let flags = flags(timeline);
    if !timeline.activity.is_empty() || !timeline.health.is_empty() {
        let _ = writeln!(
            out,
            "battery flags: {}",
            if flags.is_empty() {
                "none".into()
            } else {
                flags.join("; ")
            }
        );
    }
    if !timeline.health.is_empty() {
        health(timeline, &mut out);
    }
    out
}

fn activity(timeline: &Timeline, out: &mut String) {
    let cpu = timeline
        .activity
        .iter()
        .all(|window| window.activity.awake_us.is_some());
    let mut by_state: BTreeMap<State, Totals> = BTreeMap::new();
    for (window, state) in timeline.activity.iter().zip(states(timeline)) {
        by_state
            .entry(state)
            .or_insert_with(|| Totals::new(cpu))
            .add(window);
    }
    let simulated = timeline.activity.iter().any(|w| w.activity.simulated);
    let _ = writeln!(
        out,
        "activity by device state ({}):",
        if simulated {
            "simulator: wakeups are its passes, and it has no CPU times"
        } else {
            "CPU as a share of the time, an interrupt in what it interrupted"
        }
    );
    for (state, totals) in &by_state {
        let mut line = format!(
            "  {} for {}: {} executor wakeups",
            state.name(),
            duration(totals.ms),
            rate(totals.per_second(totals.wakes)),
        );
        if let Some(cpu) = totals.cpu {
            let _ = write!(
                line,
                ", {} timer ticks; executor awake {}, radio drivers {}, idle {}, counting {}",
                rate(totals.per_second(cpu.ticks)),
                percent(totals.share(cpu.awake)),
                percent(totals.share(cpu.radio)),
                percent(totals.share(cpu.idle)),
                percent(totals.share(cpu.hooks)),
            );
        }
        let _ = writeln!(out, "{line}");
    }
    let mut names: Vec<&String> = by_state.values().flat_map(|t| t.tasks.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let rows: Vec<String> = by_state
            .iter()
            .filter_map(|(state, totals)| {
                let task = totals.tasks.get(name)?;
                Some(format!("{} {}", state.name(), totals.task(*task)))
            })
            .collect();
        let _ = writeln!(out, "  task {name}: {}", rows.join(", "));
    }
}

/// What a dark screen kept busy, memory that keeps falling, error counters
/// that rose and passes that stalled, one phrase each.
fn flags(timeline: &Timeline) -> Vec<String> {
    let mut flags = Vec::new();
    let cpu = timeline
        .activity
        .iter()
        .all(|window| window.activity.awake_us.is_some());
    let mut dark = Totals::new(cpu);
    for window in &timeline.activity {
        if window.activity.consumers.backlight == 0 {
            dark.add(window);
        }
    }
    if dark.ms > 0 {
        for (name, task) in &dark.tasks {
            let wakes = dark.per_second(task.polls);
            let busy = task.busy.map(|busy| dark.share(busy));
            if wakes >= DARK_WAKES_PER_S || busy.is_some_and(|share| share >= DARK_CPU) {
                flags.push(format!(
                    "{name} keeps {} while the screen is dark ({})",
                    dark.task(*task),
                    duration(dark.ms)
                ));
            }
        }
        if let Some(cpu) = dark.cpu
            && dark.share(cpu.radio) >= DARK_CPU
        {
            flags.push(format!(
                "radio drivers take {} while the screen is dark",
                percent(dark.share(cpu.radio))
            ));
        }
    }
    let heap: Vec<(u64, u32)> = timeline
        .health
        .iter()
        .filter_map(|(at, h)| Some((*at, h.heap_free?)))
        .collect();
    if let Some(slope) = slope(&heap)
        && let (Some(first), Some(last)) = (heap.first(), heap.last())
        && slope * (last.0 - first.0) as f64 <= -FALLING_BYTES
    {
        flags.push(format!(
            "heap falling {} an hour",
            bytes((-slope * 3_600_000.0) as u64)
        ));
    }
    for (name, rose) in rising(&timeline.health) {
        flags.push(format!(
            "{name} rose by {} ({}-{})",
            rose.by,
            session_clock(rose.first_ms),
            session_clock(rose.last_ms)
        ));
    }
    let stalls = stalls(&timeline.health);
    if let Some((at, us)) = stalls.iter().max_by_key(|(_, us)| *us) {
        flags.push(format!(
            "{} over {} ms, the slowest {} at {}",
            match stalls.len() {
                1 => "1 pass".to_string(),
                n => format!("{n} passes"),
            },
            STALL_US / 1000,
            millis(*us),
            session_clock(*at)
        ));
    }
    flags
}

/// The least-squares trend of `points` in units a millisecond; none for
/// fewer than two distinct times.
fn slope(points: &[(u64, u32)]) -> Option<f64> {
    let n = points.len() as f64;
    let mean_t = points.iter().map(|(t, _)| *t as f64).sum::<f64>() / n;
    let mean_v = points.iter().map(|(_, v)| f64::from(*v)).sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (t, v) in points {
        let dt = *t as f64 - mean_t;
        num += dt * (f64::from(*v) - mean_v);
        den += dt * dt;
    }
    (den > 0.0).then(|| num / den)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rose {
    by: u32,
    first_ms: u64,
    last_ms: u64,
}

/// Each error counter that rose over the session, by how much and between
/// which records.
fn rising(health: &[(u64, Health)]) -> Vec<(&'static str, Rose)> {
    type Counter = (&'static str, fn(&Health) -> Option<u32>);
    let counters: [Counter; 4] = [
        ("gps receive overflows", |h| Some(h.gps_overflows)),
        ("gps uart errors", |h| Some(h.gps_uart_errors)),
        ("companion errors", |h| Some(h.companion_errors)),
        ("card recoveries", |h| h.card_recoveries),
    ];
    counters
        .into_iter()
        .filter_map(|(name, read)| {
            let mut rose: Option<Rose> = None;
            let mut values = health.iter().filter_map(|(at, h)| Some((*at, read(h)?)));
            let (_, mut before) = values.next()?;
            for (at, value) in values {
                if value > before {
                    let by = value - before;
                    rose = Some(match rose {
                        Some(r) => Rose {
                            by: r.by + by,
                            last_ms: at,
                            ..r
                        },
                        None => Rose {
                            by,
                            first_ms: at,
                            last_ms: at,
                        },
                    });
                }
                before = value;
            }
            Some((name, rose?))
        })
        .collect()
}

/// Each record whose slowest pass was a stall, with its time.
fn stalls(health: &[(u64, Health)]) -> Vec<(u64, u32)> {
    health
        .iter()
        .filter_map(|(at, h)| Some((*at, h.pass_max_us?)))
        .filter(|(_, us)| *us >= STALL_US)
        .collect()
}

fn health(timeline: &Timeline, out: &mut String) {
    let records = &timeline.health;
    let simulated = records.iter().any(|(_, h)| h.simulated);
    let _ = writeln!(
        out,
        "health, {} records{}:",
        records.len(),
        if simulated {
            " (simulator: it cannot know memory, timings or signal)"
        } else {
            ""
        }
    );
    let trend = |name: &str, read: fn(&Health) -> Option<u32>| -> Option<String> {
        let points: Vec<(u64, u32)> = records
            .iter()
            .filter_map(|(at, h)| Some((*at, read(h)?)))
            .collect();
        let (first, last) = (points.first()?, points.last()?);
        let least = points.iter().map(|(_, v)| *v).min()?;
        let per_hour = slope(&points).map_or(0.0, |s| s * 3_600_000.0);
        Some(format!(
            "{name} {} at {} -> {} at {}, least {}, {}",
            bytes(u64::from(first.1)),
            session_clock(first.0),
            bytes(u64::from(last.1)),
            session_clock(last.0),
            bytes(u64::from(least)),
            if per_hour.abs() < 1.0 {
                "steady".into()
            } else if per_hour < 0.0 {
                format!("trend -{} an hour", bytes(-per_hour as u64))
            } else {
                format!("trend +{} an hour", bytes(per_hour as u64))
            }
        ))
    };
    let memory: Vec<String> = [
        trend("heap", |h| h.heap_free),
        trend("psram", |h| h.psram_free),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !memory.is_empty() {
        let _ = writeln!(out, "  memory: {}", memory.join("; "));
    }
    let slowest = |read: fn(&Health) -> Option<u32>| {
        records
            .iter()
            .filter_map(|(at, h)| Some((*at, read(h)?)))
            .max_by_key(|(_, us)| *us)
            .map(|(at, us)| format!("{} at {}", millis(us), session_clock(at)))
    };
    let timings: Vec<String> = [
        ("pass", slowest(|h| h.pass_max_us)),
        ("frame", slowest(|h| h.frame_max_us)),
        ("card write", slowest(|h| h.card_write_max_us)),
    ]
    .into_iter()
    .filter_map(|(name, slowest)| Some(format!("slowest {name} {}", slowest?)))
    .collect();
    if !timings.is_empty() {
        let _ = writeln!(out, "  timing: {}", timings.join("; "));
    }
    let rose = rising(records);
    let _ = writeln!(
        out,
        "  errors: {}",
        if rose.is_empty() {
            "no counter rose".into()
        } else {
            rose.iter()
                .map(|(name, r)| format!("{name} +{}", r.by))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    if let Some((at, h)) = records
        .iter()
        .filter(|(_, h)| h.spool_size > 0)
        .max_by_key(|(_, h)| h.spool_used)
    {
        let _ = writeln!(
            out,
            "  spool: fullest {} of {} at {}",
            bytes(u64::from(h.spool_used)),
            bytes(u64::from(h.spool_size)),
            session_clock(*at)
        );
    }
    let signal = |name: &str, read: fn(&Health) -> Option<i8>| {
        let (least, most) = records.iter().filter_map(|(_, h)| read(h)).fold(
            (None::<i8>, None::<i8>),
            |(least, most), v| {
                (
                    Some(least.map_or(v, |l| l.min(v))),
                    Some(most.map_or(v, |m| m.max(v))),
                )
            },
        );
        Some(format!("{name} {} to {} dBm", least?, most?))
    };
    let signals: Vec<String> = [
        signal("wifi", |h| h.wifi_rssi),
        signal("ble sensor", |h| h.ble_rssi),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !signals.is_empty() {
        let _ = writeln!(out, "  signal: {}", signals.join("; "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::timeline::read;
    use firmware_services::trace::frozen::{Record, Recorder, Sink};
    use firmware_services::trace::{self, Activity, Consumers, Event, Header};

    struct Blocks(Vec<u8>);

    impl Sink for Blocks {
        fn block(&mut self, bytes: &[u8]) -> bool {
            self.0.extend_from_slice(bytes);
            true
        }
    }

    /// A device trace of `passes`, each a time and the events in it.
    fn timeline(passes: &[(u64, Vec<Event<'_>>)]) -> Timeline {
        let header = Header {
            version: trace::VERSION,
            oldest_reader: trace::OLDEST_READER,
            build: [0; 20],
            boot: 1,
            profile: 0,
            config: &[],
            media: &[],
        };
        let mut file = vec![0u8; 4096];
        let len = trace::encode_header(&header, &mut file).unwrap();
        file.truncate(len);
        let mut recorder = Recorder::new(vec![0u8; 4096].into_boxed_slice(), Blocks(file));
        for (now_ms, events) in passes {
            recorder.record(&Event::Pass { now_ms: *now_ms });
            for event in events {
                recorder.record(event);
            }
        }
        recorder.flush();
        read(&recorder.sink().0).expect("the trace decodes")
    }

    /// Five seconds of a device: the executor woke 1,200 times and ran
    /// 2%, the radio drivers 0.5%, idle the rest.
    fn window(backlight: u8, console: (u32, u32)) -> Vec<Event<'static>> {
        vec![
            Event::Activity(Activity {
                simulated: false,
                window_ms: 5_000,
                wakes: 1_200,
                awake_us: Some(100_000),
                main_us: Some(110_000),
                radio_us: Some(25_000),
                idle_us: Some(4_850_000),
                switches: Some(2_500),
                ticks: Some(1_300),
                hooks_us: Some(1_000),
                consumers: Consumers {
                    cpu_mhz: Some(240),
                    backlight,
                    ..Consumers::default()
                },
            }),
            Event::Task {
                name: "console",
                polls: console.0,
                busy_us: Some(console.1),
            },
            Event::Task {
                name: "io",
                polls: 500,
                busy_us: Some(5_000),
            },
        ]
    }

    fn charging() -> Event<'static> {
        Event::PowerControl(device_api::power::Status {
            sequence: 1,
            operation: None,
            state: Power::Charging,
            at_ms: 0,
            prepare_at_ms: None,
            failure: None,
            ready: false,
        })
    }

    #[test]
    fn each_state_adds_up_its_windows_and_a_dark_screen_flags_what_kept_waking() {
        let timeline = timeline(&[
            (5_000, window(80, (1_000, 50_000))),
            (10_000, window(80, (1_000, 50_000))),
            (12_000, vec![charging()]),
            (15_000, window(0, (1_000, 100_000))),
        ]);
        let summary = summary(&timeline);
        assert!(
            summary.contains(
                "  home idle for 10.0s: 240/s executor wakeups, 260/s timer ticks; \
                 executor awake 2.0%, radio drivers 0.5%, idle 97.0%, counting 0.0%"
            ),
            "{summary}"
        );
        assert!(
            summary.contains("  task console: home idle 200/s 1.0%, charging standby 200/s 2.0%"),
            "{summary}"
        );
        assert!(
            summary.contains(
                "battery flags: console keeps 200/s 2.0% while the screen is dark (5.0s); \
                 io keeps 100/s 0.1% while the screen is dark (5.0s)"
            ),
            "{summary}"
        );
    }

    #[test]
    fn a_task_that_sleeps_while_the_screen_is_dark_is_not_flagged() {
        let timeline = timeline(&[(5_000, window(0, (20, 2_000)))]);
        let flags = flags(&timeline);
        assert!(
            flags.iter().all(|flag| !flag.starts_with("console")),
            "{flags:?}"
        );
    }

    fn health(at_s: u64, heap: u32, overflows: u32, pass_us: u32) -> (u64, Vec<Event<'static>>) {
        let health = Health {
            heap_free: Some(heap),
            heap_min: Some(heap),
            pass_max_us: Some(pass_us),
            gps_overflows: overflows,
            ..Health::default()
        };
        (at_s * 1000, vec![Event::Health(health)])
    }

    #[test]
    fn a_falling_heap_rising_counters_and_stalls_are_flagged() {
        let passes: Vec<_> = (1..=120)
            .map(|i| {
                let overflows = if i < 40 {
                    0
                } else if i < 90 {
                    2
                } else {
                    3
                };
                let pass_us = if i == 70 { 180_000 } else { 9_000 };
                health(i * 5, 60_000 - i as u32 * 20, overflows, pass_us)
            })
            .collect();
        let timeline = timeline(&passes);
        assert_eq!(
            flags(&timeline),
            [
                "heap falling 14.1 KiB an hour",
                "gps receive overflows rose by 3 (03:20.000-07:30.000)",
                "1 pass over 100 ms, the slowest 180.0 ms at 05:50.000",
            ]
        );
        let summary = summary(&timeline);
        assert!(
            summary.contains("  errors: gps receive overflows +3"),
            "{summary}"
        );
        assert!(
            summary.contains("  timing: slowest pass 180.0 ms at 05:50.000"),
            "{summary}"
        );
    }

    #[test]
    fn a_heap_that_wanders_around_one_level_is_steady() {
        let passes: Vec<_> = (1..=120)
            .map(|i| health(i * 5, 60_000 + (i % 7) as u32 * 300, 0, 9_000))
            .collect();
        assert_eq!(flags(&timeline(&passes)), Vec::<String>::new());
    }

    #[test]
    fn a_trace_from_before_health_and_activity_says_nothing_about_them() {
        let timeline = timeline(&[(10, vec![]), (20, vec![])]);
        assert_eq!(summary(&timeline), "");
    }
}
