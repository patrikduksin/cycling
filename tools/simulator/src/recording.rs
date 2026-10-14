//! Scenario recordings and the artifacts a reviewer reads: a captioned video,
//! checkpoint screenshots, a chart of the world, recorded rides and a report.
//!
//! Recording is enabled by `SIM_ARTIFACTS=<directory>`; each scenario writes
//! `<directory>/<scenario>/` when its `Sim` is dropped, including on failure.
use crate::artifacts::{Canvas, Series, Still, chart_png, chart_svg, gif, mp4};
use crate::sim::{Note, NoteKind};
use crate::world::World;
use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Device screens are shown at twice the panel size.
const SCALE: usize = 2;
const PANEL_WIDTH: usize = 360;
const MARGIN: usize = 20;
/// Moments around an action play in real time; the rest is compressed so the
/// whole video stays short.
const REAL_TIME_BEFORE_MS: u64 = 500;
const REAL_TIME_AFTER_MS: u64 = 3_000;
const TARGET_VIDEO_MS: u64 = 45_000;
const MAX_REAL_TIME_MS: u64 = 90_000;
const FPS: u32 = 20;

const BACKGROUND: [u8; 3] = [0x14, 0x18, 0x1d];
const BEZEL: [u8; 3] = [0x2b, 0x32, 0x3a];
const INK: [u8; 3] = [0xc9, 0xd1, 0xd9];
const DIM: [u8; 3] = [0x76, 0x80, 0x8a];
const ACCENT: [u8; 3] = [0x00, 0xd7, 0xd7];
const AMBER: [u8; 3] = [0xd2, 0x99, 0x22];
const PASS: [u8; 3] = [0x3f, 0xb9, 0x50];
const FAIL: [u8; 3] = [0xf8, 0x51, 0x49];

thread_local! {
    /// The panic message of the current test thread, for failure reports.
    static PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// A directory name no other recording in this process uses.
fn claim(name: String) -> String {
    static CLAIMED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut claimed = CLAIMED.lock().unwrap_or_else(|e| e.into_inner());
    let unique = (1..)
        .map(|n| {
            if n == 1 {
                name.clone()
            } else {
                format!("{name}-{n}")
            }
        })
        .find(|candidate| !claimed.contains(candidate))
        .expect("an unused name exists");
    claimed.push(unique.clone());
    unique
}

/// Remember panic messages per thread, then defer to the default hook.
pub fn capture_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            let location = info
                .location()
                .map(|l| format!(" at {}:{}", l.file(), l.line()))
                .unwrap_or_default();
            PANIC.with(|p| *p.borrow_mut() = Some(format!("{message}{location}")));
            default(info);
        }));
    });
}

/// A frame as runs of equal pixels; UI frames compress well.
struct Frame {
    at_ms: u64,
    runs: Vec<(u16, u32)>,
}

impl Frame {
    fn new(at_ms: u64, pixels: &[u16]) -> Self {
        let mut runs: Vec<(u16, u32)> = Vec::new();
        for &pixel in pixels {
            match runs.last_mut() {
                Some((color, count)) if *color == pixel => *count += 1,
                _ => runs.push((pixel, 1)),
            }
        }
        Self { at_ms, runs }
    }

    fn pixels(&self) -> Vec<u16> {
        self.runs
            .iter()
            .flat_map(|&(color, count)| std::iter::repeat_n(color, count as usize))
            .collect()
    }
}

/// The world each second, for charts.
#[derive(Clone, Copy)]
struct Truth {
    at_ms: u64,
    speed_kmh: f64,
    power_w: f64,
    heart_bpm: f64,
    elevation_m: Option<f64>,
    nearest_vehicle_m: Option<f64>,
}

pub struct Checkpoint {
    pub name: String,
    pub at_ms: u64,
    frame: usize,
}

pub struct Recording {
    pub name: String,
    dir: PathBuf,
    width: usize,
    height: usize,
    frames: Vec<Frame>,
    checkpoints: Vec<Checkpoint>,
    truth: Vec<Truth>,
    submissions: u32,
    next_truth_ms: u64,
}

/// What a finished scenario hands to the report.
pub struct Outcome<'a> {
    pub notes: &'a [Note],
    pub end_ms: u64,
    pub rides: Vec<RideSummary>,
}

pub struct RideSummary {
    pub id: u32,
    pub kinds: Vec<String>,
    pub samples: usize,
    pub track_m: f64,
    pub gpx: String,
    pub json: String,
}

impl Recording {
    /// A recording for the current test thread, when artifacts are requested.
    pub fn from_env(width: usize, height: usize) -> Option<Self> {
        let root = std::env::var_os("SIM_ARTIFACTS")?;
        capture_panics();
        let thread = std::thread::current();
        let name = thread
            .name()
            .unwrap_or("scenario")
            .rsplit("::")
            .next()
            .unwrap_or("scenario")
            .to_string();
        let name = claim(name);
        PANIC.with(|p| p.borrow_mut().take());
        Some(Self {
            dir: Path::new(&root).join(&name),
            name,
            width,
            height,
            frames: Vec::new(),
            checkpoints: Vec::new(),
            truth: Vec::new(),
            submissions: u32::MAX,
            next_truth_ms: 0,
        })
    }

    /// Keep the panel when it changed since the last observation.
    pub fn observe(&mut self, at_ms: u64, submissions: u32, pixels: &[u16], world: &World) {
        if submissions != self.submissions {
            self.submissions = submissions;
            let frame = Frame::new(at_ms, pixels);
            if self
                .frames
                .last()
                .is_none_or(|last| last.runs != frame.runs)
            {
                self.frames.push(frame);
            }
        }
        if at_ms >= self.next_truth_ms {
            self.next_truth_ms = at_ms - at_ms % 1000 + 1000;
            self.truth.push(Truth {
                at_ms,
                speed_kmh: world.speed_kmh,
                power_w: world.power_w,
                heart_bpm: world.heart_bpm,
                elevation_m: world.elevation_m,
                nearest_vehicle_m: world
                    .vehicles
                    .iter()
                    .map(|v| v.range_m)
                    .min_by(f64::total_cmp),
            });
        }
    }

    pub fn checkpoint(&mut self, name: &str, at_ms: u64, pixels: &[u16]) {
        self.frames.push(Frame::new(at_ms, pixels));
        self.checkpoints.push(Checkpoint {
            name: name.to_string(),
            at_ms,
            frame: self.frames.len() - 1,
        });
    }

    /// Write every artifact; failures to write are reported, never hidden.
    pub fn write(&self, outcome: Outcome<'_>) -> io::Result<()> {
        let failure = std::thread::panicking()
            .then(|| PANIC.with(|p| p.borrow_mut().take()).unwrap_or_default());
        if self.dir.exists() {
            fs::remove_dir_all(&self.dir)?;
        }
        fs::create_dir_all(&self.dir)?;
        let mut files = Vec::new();

        let stills = self.stills(&outcome, failure.as_deref());
        if !stills.is_empty() {
            if mp4(&stills, FPS, &self.dir.join("video.mp4"))? {
                files.push("video.mp4".to_string());
            } else {
                fs::write(self.dir.join("video.gif"), gif(&stills))?;
                files.push("video.gif".to_string());
            }
        }
        for (index, checkpoint) in self.checkpoints.iter().enumerate() {
            let canvas = self.compose(
                checkpoint.frame,
                checkpoint.at_ms,
                &outcome,
                failure.as_deref(),
                Some(&checkpoint.name),
            );
            let file = format!("{:02}-{}.png", index + 1, slug(&checkpoint.name));
            fs::write(self.dir.join(&file), canvas.png())?;
            files.push(file);
        }
        if let Some(index) = self.frames.len().checked_sub(1) {
            let canvas = self.compose(
                index,
                outcome.end_ms,
                &outcome,
                failure.as_deref(),
                Some(if failure.is_some() {
                    "failed here"
                } else {
                    "final screen"
                }),
            );
            fs::write(self.dir.join("final.png"), canvas.png())?;
            files.push("final.png".to_string());
        }
        let (series, markers) = self.series(&outcome);
        if !series.is_empty() {
            fs::write(
                self.dir.join("chart.svg"),
                chart_svg(&series, &markers, 900),
            )?;
            fs::write(
                self.dir.join("chart.png"),
                chart_png(&series, &markers, 900).png(),
            )?;
            files.push("chart.png".to_string());
        }
        for ride in &outcome.rides {
            fs::write(self.dir.join(format!("ride-{}.gpx", ride.id)), &ride.gpx)?;
            fs::write(self.dir.join(format!("ride-{}.json", ride.id)), &ride.json)?;
        }
        fs::write(self.dir.join("timeline.txt"), timeline(outcome.notes))?;
        fs::write(
            self.dir.join("result.json"),
            self.result(&outcome, failure.as_deref(), &files),
        )?;
        Ok(())
    }

    /// The video: every distinct frame, held for its compressed duration.
    fn stills(&self, outcome: &Outcome<'_>, failure: Option<&str>) -> Vec<Still> {
        let Some(first) = self.frames.first() else {
            return Vec::new();
        };
        let start = first.at_ms;
        let end = outcome
            .end_ms
            .max(self.frames.last().map_or(start, |f| f.at_ms) + 1000);
        // Windows around actions play in real time; a frame counts only the
        // part of its hold that overlaps them, so long quiet holds compress.
        let mut windows: Vec<(u64, u64)> = outcome
            .notes
            .iter()
            .filter(|note| note.kind != NoteKind::Check)
            .map(|note| (note.at_ms, REAL_TIME_AFTER_MS))
            .chain(self.checkpoints.iter().map(|c| (c.at_ms, 1500)))
            .map(|(at, after)| (at.saturating_sub(REAL_TIME_BEFORE_MS), at + after))
            .collect();
        windows.sort_unstable();
        let overlap = |from: u64, until: u64| -> u64 {
            let mut covered = 0;
            let mut reached = from;
            for &(start, stop) in &windows {
                let (start, stop) = (start.max(reached), stop.min(until));
                if start < stop {
                    covered += stop - start;
                    reached = stop;
                }
            }
            covered
        };
        let durations: Vec<(usize, u64, u64)> = (0..self.frames.len())
            .map(|i| {
                let from = self.frames[i].at_ms;
                let until = self.frames.get(i + 1).map_or(end, |f| f.at_ms).max(from);
                (i, until - from, overlap(from, until))
            })
            .filter(|&(_, ms, _)| ms > 0)
            .collect();
        let real: u64 = durations.iter().map(|&(_, _, real)| real).sum();
        let compressed: u64 = durations.iter().map(|&(_, ms, _)| ms).sum::<u64>() - real;
        // Real time plays at full speed up to the limit, then everything speeds up.
        let real_speed = (real as f64 / MAX_REAL_TIME_MS as f64).max(1.0);
        let shown_real = real as f64 / real_speed;
        let budget = (TARGET_VIDEO_MS as f64 - shown_real).max(10_000.0);
        let speed = (compressed as f64 / budget).max(1.0);
        let mut stills = Vec::new();
        let mut carried = 0.0;
        for (index, ms, real) in durations {
            let at = self.frames[index].at_ms;
            let shown = real as f64 / real_speed + (ms - real) as f64 / speed + carried;
            // Frames shorter than one video frame merge into the next.
            if shown < 1000.0 / f64::from(FPS) && !self.checkpoints.iter().any(|c| c.frame == index)
            {
                carried = shown;
                continue;
            }
            carried = 0.0;
            let caption = self
                .checkpoints
                .iter()
                .find(|c| c.frame == index)
                .map(|c| c.name.as_str());
            stills.push(Still {
                canvas: self.compose(index, at, outcome, failure, caption),
                hold_ms: shown as u32,
            });
        }
        stills
    }

    /// The device screen with a panel of time, scenario and recent events.
    fn compose(
        &self,
        index: usize,
        at_ms: u64,
        outcome: &Outcome<'_>,
        failure: Option<&str>,
        caption: Option<&str>,
    ) -> Canvas {
        let screen_w = self.width * SCALE;
        let screen_h = self.height * SCALE;
        let width = MARGIN * 3 + screen_w + PANEL_WIDTH;
        let height = MARGIN * 2 + screen_h + 40;
        let mut canvas = Canvas::new(width, height, BACKGROUND);
        canvas.fill_rect(MARGIN - 6, MARGIN - 6, screen_w + 12, screen_h + 12, BEZEL);
        canvas.blit565(
            MARGIN,
            MARGIN,
            self.width,
            self.height,
            &self.frames[index].pixels(),
            SCALE,
        );
        let x = MARGIN * 2 + screen_w;
        let mut y = MARGIN;
        y += wrap(&mut canvas, x, y, 2, INK, &title(&self.name), PANEL_WIDTH) + 10;
        canvas.text(x, y, 3, ACCENT, &format!("T+{}", clock(at_ms)));
        y += 36;
        if let Some(caption) = caption {
            y += wrap(
                &mut canvas,
                x,
                y,
                2,
                AMBER,
                &format!("> {caption}"),
                PANEL_WIDTH,
            ) + 8;
        }
        if let Some(truth) = self.truth.iter().rev().find(|t| t.at_ms <= at_ms) {
            let line = format!(
                "{:.0} KM/H  {:.0} W  {:.0} BPM",
                truth.speed_kmh, truth.power_w, truth.heart_bpm
            );
            canvas.text(x, y, 1, DIM, "WORLD");
            canvas.text(x, y + 10, 2, INK, &line);
            y += 36;
        }
        canvas.text(x, y, 1, DIM, "EVENTS");
        y += 12;
        let recent: Vec<_> = outcome
            .notes
            .iter()
            .filter(|n| n.at_ms <= at_ms)
            .rev()
            .take(12)
            .collect();
        for note in recent.iter().rev() {
            if y + 20 > height - 40 {
                break;
            }
            let fresh = at_ms.saturating_sub(note.at_ms) < REAL_TIME_AFTER_MS;
            let color = match (note.kind, fresh) {
                (NoteKind::Check, _) => PASS,
                (_, true) => AMBER,
                _ => DIM,
            };
            let line = format!("{} {}", clock(note.at_ms), note.text);
            y += wrap(&mut canvas, x, y, 1, color, &line, PANEL_WIDTH) + 4;
        }
        let status_y = height - 30;
        match failure {
            Some(message) => {
                canvas.text(MARGIN, status_y, 2, FAIL, "FAILED");
                canvas.text(MARGIN + 90, status_y + 4, 1, FAIL, &one_line(message, 120));
            }
            None => {
                canvas.text(MARGIN, status_y, 2, DIM, "VANA ON SIMULATED C606");
            }
        }
        canvas
    }

    fn series(&self, outcome: &Outcome<'_>) -> (Vec<Series>, Vec<(u64, String)>) {
        let line = |name: &str, unit: &str, color: &str, value: fn(&Truth) -> Option<f64>| Series {
            name: name.into(),
            unit: unit.into(),
            color: color.into(),
            points: self.truth.iter().map(|t| (t.at_ms, value(t))).collect(),
        };
        let mut series = vec![
            line("Speed", "km/h", "#00d7d7", |t| Some(t.speed_kmh)),
            line("Power", "W", "#d29922", |t| Some(t.power_w)),
            line("Heart rate", "bpm", "#f85149", |t| Some(t.heart_bpm)),
        ];
        if self.truth.iter().any(|t| t.elevation_m.is_some()) {
            series.push(line("Elevation", "m", "#3fb950", |t| t.elevation_m));
        }
        if self.truth.iter().any(|t| t.nearest_vehicle_m.is_some()) {
            series.push(line("Nearest vehicle", "m", "#a371f7", |t| {
                t.nearest_vehicle_m
            }));
        }
        series.retain(|s| s.points.iter().any(|p| p.1.is_some_and(|v| v != 0.0)));
        let markers = outcome
            .notes
            .iter()
            .filter(|n| matches!(n.kind, NoteKind::World | NoteKind::Device))
            .map(|n| (n.at_ms, n.text.clone()))
            .collect();
        (series, markers)
    }

    fn result(&self, outcome: &Outcome<'_>, failure: Option<&str>, files: &[String]) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "{{\n  \"name\": {},\n  \"status\": \"{}\",\n  \"failure\": {},\n  \"simulated_ms\": {},\n  \"frames\": {},\n  \"files\": [{}],\n  \"checkpoints\": [{}],\n  \"rides\": [{}]\n}}\n",
            json(&self.name),
            if failure.is_some() { "fail" } else { "pass" },
            failure.map_or("null".to_string(), json),
            outcome.end_ms,
            self.frames.len(),
            files.iter().map(|f| json(f)).collect::<Vec<_>>().join(", "),
            self.checkpoints
                .iter()
                .map(|c| json(&c.name))
                .collect::<Vec<_>>()
                .join(", "),
            outcome
                .rides
                .iter()
                .map(|r| format!(
                    "{{\"id\": {}, \"samples\": {}, \"track_m\": {:.0}, \"entries\": [{}]}}",
                    r.id,
                    r.samples,
                    if r.track_m.is_finite() {
                        r.track_m
                    } else {
                        0.0
                    },
                    r.kinds
                        .iter()
                        .map(|k| json(k))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out
    }
}

/// Draw text wrapped to a width; returns the height used.
fn wrap(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    scale: usize,
    color: [u8; 3],
    text: &str,
    width: usize,
) -> usize {
    let per_line = (width / (6 * scale)).max(1);
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() && line.len() + 1 + word.len() > per_line {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    for (i, line) in lines.iter().enumerate() {
        canvas.text(x, y + i * 10 * scale, scale, color, line);
    }
    lines.len() * 10 * scale
}

pub fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!(
            "{:02}:{:02}.{}",
            seconds / 60,
            seconds % 60,
            ms % 1000 / 100
        )
    }
}

fn title(name: &str) -> String {
    name.replace('_', " ")
}

pub fn slug(text: &str) -> String {
    text.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn one_line(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    line.chars().take(limit).collect()
}

fn timeline(notes: &[Note]) -> String {
    let mut out = String::new();
    for note in notes {
        let _ = writeln!(out, "{} {:?} {}", clock(note.at_ms), note.kind, note.text);
    }
    out
}

pub fn json(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_through_runs() {
        let pixels = [1, 1, 1, 2, 3, 3, 0xffff];
        let frame = Frame::new(0, &pixels);
        assert_eq!(frame.runs.len(), 4);
        assert_eq!(frame.pixels(), pixels);
    }

    #[test]
    fn names_and_text_are_safe_for_files_and_json() {
        assert_eq!(slug("Paired: HR + power!"), "paired-hr-power");
        assert_eq!(json("a \"b\"\n\\"), "\"a \\\"b\\\"\\n\\\\\"");
        assert_eq!(clock(65_400), "01:05.4");
        assert_eq!(clock(3_723_000), "1:02:03");
    }

    fn recording(frames: &[u64]) -> Recording {
        let mut recording = Recording {
            name: "timing".into(),
            dir: PathBuf::new(),
            width: 4,
            height: 2,
            frames: Vec::new(),
            checkpoints: Vec::new(),
            truth: Vec::new(),
            submissions: u32::MAX,
            next_truth_ms: 0,
        };
        for (i, &at) in frames.iter().enumerate() {
            recording.frames.push(Frame::new(at, &[i as u16; 8]));
        }
        recording
    }

    fn note(at_ms: u64) -> Note {
        Note {
            at_ms,
            kind: NoteKind::Input,
            text: "press".into(),
        }
    }

    fn video_ms(recording: &Recording, notes: &[Note], end_ms: u64) -> u64 {
        let outcome = Outcome {
            notes,
            end_ms,
            rides: Vec::new(),
        };
        let stills = recording.stills(&outcome, None);
        stills.iter().map(|s| u64::from(s.hold_ms)).sum()
    }

    #[test]
    fn a_long_hold_after_an_action_plays_only_its_window_in_real_time() {
        // A press, then ten unchanged minutes on one screen.
        let recording = recording(&[0, 100]);
        let total = video_ms(&recording, &[note(0)], 600_000);
        assert!(
            (3_000..=TARGET_VIDEO_MS + 1_000).contains(&total),
            "{total} ms"
        );
    }

    #[test]
    fn many_actions_still_fit_the_real_time_limit() {
        let frames: Vec<u64> = (0..2_000).map(|i| i * 1_000).collect();
        let notes: Vec<Note> = (0..2_000).map(|i| note(i * 1_000)).collect();
        let total = video_ms(&recording(&frames), &notes, 2_000_000);
        assert!(total <= MAX_REAL_TIME_MS + 1_000, "{total} ms");
    }
}
