//! Scenario recordings and the artifacts a reviewer reads: a captioned video,
//! checkpoint screenshots, a chart of the world, recorded rides and a report.
//!
//! Recording is enabled by `SIM_ARTIFACTS=<directory>`; each scenario writes
//! `<directory>/<scenario>/` when its `Sim` is dropped, including on failure.
//! The video is written only with `SIM_VIDEO=1`, but every scenario stores the
//! frames it is made from in `frames.bin`, so [`render_video`] can make it
//! later without running the scenario again.
use crate::artifacts::{Canvas, Series, Still, chart_png, chart_svg, gif, mp4};
use crate::devices::Model;
use crate::sim::{Note, NoteKind};
use crate::world::World;
use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tape::{Panels, Playback, Shot};

mod tape;

/// Device screens are shown at twice the panel size.
pub(crate) const SCALE: usize = 2;
pub(crate) const PANEL_WIDTH: usize = 360;
pub(crate) const MARGIN: usize = 20;
/// Moments around an action play in real time; the rest is compressed so the
/// whole video stays short.
const REAL_TIME_BEFORE_MS: u64 = 500;
const REAL_TIME_AFTER_MS: u64 = 3_000;
const TARGET_VIDEO_MS: u64 = 45_000;
const MAX_REAL_TIME_MS: u64 = 90_000;
const FPS: u32 = 20;

pub(crate) const BACKGROUND: [u8; 3] = [0x14, 0x18, 0x1d];
pub(crate) const BEZEL: [u8; 3] = [0x2b, 0x32, 0x3a];
pub(crate) const INK: [u8; 3] = [0xc9, 0xd1, 0xd9];
pub(crate) const DIM: [u8; 3] = [0x76, 0x80, 0x8a];
pub(crate) const ACCENT: [u8; 3] = [0x00, 0xd7, 0xd7];
pub(crate) const AMBER: [u8; 3] = [0xd2, 0x99, 0x22];
pub(crate) const PASS: [u8; 3] = [0x3f, 0xb9, 0x50];
pub(crate) const FAIL: [u8; 3] = [0xf8, 0x51, 0x49];

thread_local! {
    /// The panic message of the current test thread, for failure reports.
    static PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn claim(name: &str, device: &str) -> (String, String) {
    static CLAIMED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut claimed = CLAIMED.lock().unwrap_or_else(|e| e.into_inner());
    let (name, dir) = (1..)
        .map(|n| {
            let name = if n == 1 {
                name.to_string()
            } else {
                format!("{name}-{n}")
            };
            let dir = format!("{name}@{device}");
            (name, dir)
        })
        .find(|(_, dir)| !claimed.contains(dir))
        .expect("an unused name exists");
    claimed.push(dir.clone());
    (name, dir)
}

/// The file that marks a directory as the simulator's own output.
pub const MARKER: &str = ".simulator-output";

/// Make `dir` an empty directory marked as the simulator's output. A
/// directory is replaced only when it holds [`MARKER`] or is empty; anything
/// else is refused, so a mistyped path never loses someone's files. The
/// directory the process runs in, and any directory above it, is never
/// claimed: an empty `--out` would otherwise mark it, and a later `--out .`
/// would empty it.
pub fn replace_dir(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Err(io::Error::other("the output directory is empty; name one"));
    }
    let located =
        |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", dir.display()));
    fs::create_dir_all(dir).map_err(located)?;
    let real = dir.canonicalize().map_err(located)?;
    if std::env::current_dir()?.canonicalize()?.starts_with(&real) {
        return Err(io::Error::other(format!(
            "{} is this directory or one above it; choose a directory inside it",
            dir.display()
        )));
    }
    let mut entries = fs::read_dir(&real).map_err(located)?;
    if !real.join(MARKER).is_file() && entries.next().is_some() {
        return Err(io::Error::other(format!(
            "{} holds files the simulator did not write (it has no {MARKER}); \
             choose another directory, or empty this one yourself",
            dir.display()
        )));
    }
    fs::remove_dir_all(&real)?;
    fs::create_dir_all(&real)?;
    fs::write(
        real.join(MARKER),
        "Written by the simulator; the next run into this directory replaces it.\n",
    )
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
                .map(|l| format!(" at {}:{}", portable(l.file()), l.line()))
                .unwrap_or_default();
            PANIC.with(|p| *p.borrow_mut() = Some(format!("{message}{location}")));
            default(info);
        }));
    });
}

/// `file` as a panic location names it on any machine: a workspace file is
/// already relative to the workspace, a dependency's is cut to its crate
/// directory and the standard library's to `library/`, so the machine's
/// cargo home and the toolchain's commit drop out.
fn portable(file: &str) -> &str {
    if !file.starts_with('/') {
        return file;
    }
    for marker in ["/registry/src/", "/git/checkouts/"] {
        if let Some((_, under)) = file.split_once(marker) {
            return under.split_once('/').map_or(under, |(_, path)| path);
        }
    }
    if let Some(at) = file.find("/library/") {
        return &file[at + 1..];
    }
    file.rsplit('/').next().unwrap_or(file)
}

/// The message of this thread's last panic, once.
pub fn take_panic() -> Option<String> {
    PANIC.with(|p| p.borrow_mut().take())
}

/// Render the video of the scenario recorded in `dir` from the shots it
/// stored, as if it had run with `SIM_VIDEO=1`; returns the file written.
pub fn render_video(dir: &Path) -> io::Result<String> {
    let bytes = fs::read(dir.join(tape::FILE))?;
    let (header, film) = tape::read(&bytes)?;
    let mut recording = Recording::at(
        dir.to_path_buf(),
        &header.name,
        &header.device,
        header.width,
        header.height,
    );
    recording.truth = header.truth;
    recording.video(
        &header.shots,
        film,
        &header.notes,
        header.failure.as_deref(),
    )
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
    pixels: Vec<u16>,
    /// Notes taken before this moment; later notes at the same time are not shown.
    notes: usize,
}

pub struct Recording {
    pub name: String,
    device: String,
    dir: PathBuf,
    width: usize,
    height: usize,
    frames_at: Vec<u64>,
    panels: Panels,
    last: Vec<u16>,
    checkpoints: Vec<Checkpoint>,
    truth: Vec<Truth>,
    submissions: u32,
    next_truth_ms: u64,
    video: bool,
}

/// What a finished scenario hands to the report.
pub struct Outcome<'a> {
    pub device: &'static str,
    pub notes: &'a [Note],
    pub end_ms: u64,
    pub rides: Vec<RideSummary>,
    pub hashes: Option<crate::compare::Hashes>,
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
    pub fn from_env(model: &Model) -> Option<Self> {
        let thread = std::thread::current();
        let name = thread
            .name()
            .unwrap_or("scenario")
            .rsplit("::")
            .next()
            .unwrap_or("scenario");
        Self::named(name, model)
    }

    pub fn named(name: &str, model: &Model) -> Option<Self> {
        let root = std::env::var_os("SIM_ARTIFACTS")?;
        let video = std::env::var_os("SIM_VIDEO").is_some_and(|v| v == "1");
        let (name, dir) = claim(name, model.name);
        let recording = Self::at(
            Path::new(&root).join(dir),
            &name,
            model.name,
            model.width,
            model.height,
        );
        Some(recording.with_video(video))
    }

    pub fn at(dir: PathBuf, name: &str, device: &str, width: usize, height: usize) -> Self {
        capture_panics();
        PANIC.with(|p| p.borrow_mut().take());
        Self {
            dir,
            name: name.to_string(),
            device: device.to_string(),
            width,
            height,
            frames_at: Vec::new(),
            panels: Panels::new(),
            last: Vec::new(),
            checkpoints: Vec::new(),
            truth: Vec::new(),
            submissions: u32::MAX,
            next_truth_ms: 0,
            video: false,
        }
    }

    /// Also write the captioned video.
    pub fn with_video(mut self, video: bool) -> Self {
        self.video = video;
        self
    }

    /// Keep the panel when it changed since the last observation, and the
    /// world each second when there is one.
    pub fn observe(&mut self, at_ms: u64, submissions: u32, pixels: &[u16], world: Option<&World>) {
        if submissions != self.submissions {
            self.submissions = submissions;
            if pixels != self.last {
                self.keep(at_ms, pixels);
            }
        }
        if let Some(world) = world
            && at_ms >= self.next_truth_ms
        {
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

    pub fn checkpoint(&mut self, name: &str, at_ms: u64, pixels: &[u16], notes: usize) {
        self.keep(at_ms, pixels);
        self.checkpoints.push(Checkpoint {
            name: name.to_string(),
            at_ms,
            frame: self.frames_at.len() - 1,
            pixels: pixels.to_vec(),
            notes,
        });
    }

    fn keep(&mut self, at_ms: u64, pixels: &[u16]) {
        self.frames_at.push(at_ms);
        self.panels.push(&self.last, pixels, self.width);
        self.last.clear();
        self.last.extend_from_slice(pixels);
    }

    /// Write every artifact; failures to write are reported, never hidden.
    pub fn write(mut self, outcome: Outcome<'_>) -> io::Result<()> {
        let failure = std::thread::panicking()
            .then(|| PANIC.with(|p| p.borrow_mut().take()).unwrap_or_default());
        replace_dir(&self.dir)?;
        let mut files = Vec::new();

        let (frames, shots): (Vec<usize>, Vec<Shot>) = self.shots(&outcome).into_iter().unzip();
        if !shots.is_empty() {
            let panels = std::mem::replace(&mut self.panels, Panels::new()).finish();
            let film = tape::film(&panels, self.width, self.height, &frames)?;
            if self.video {
                files.push(self.video(&shots, &film, outcome.notes, failure.as_deref())?);
            }
            let mut truth: Vec<Truth> = shots
                .iter()
                .filter_map(|shot| self.truth_at(shot.at_ms).copied())
                .collect();
            truth.dedup_by_key(|t| t.at_ms);
            let header = tape::Header {
                name: self.name.clone(),
                device: self.device.clone(),
                width: self.width,
                height: self.height,
                failure: failure.clone(),
                notes: outcome.notes.to_vec(),
                truth,
                shots,
            };
            fs::write(self.dir.join(tape::FILE), tape::write(&header, &film))?;
        }
        for (index, checkpoint) in self.checkpoints.iter().enumerate() {
            let canvas = self.compose(
                &checkpoint.pixels,
                checkpoint.at_ms,
                &outcome.notes[..checkpoint.notes.min(outcome.notes.len())],
                failure.as_deref(),
                Some(&checkpoint.name),
            );
            let file = format!("{:02}-{}.png", index + 1, slug(&checkpoint.name));
            fs::write(self.dir.join(&file), canvas.png())?;
            files.push(file);
        }
        if !self.frames_at.is_empty() {
            let canvas = self.compose(
                &self.last,
                outcome.end_ms,
                outcome.notes,
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
        if let Some(hashes) = &outcome.hashes {
            fs::write(self.dir.join("hashes.json"), hashes.json())?;
            if let Some(run) = self.dir.parent() {
                fs::write(run.join(crate::compare::MARKER), "")?;
            }
        }
        fs::write(self.dir.join("timeline.txt"), timeline(outcome.notes))?;
        fs::write(
            self.dir.join("result.json"),
            self.result(&outcome, failure.as_deref(), &files),
        )?;
        Ok(())
    }

    fn video(
        &self,
        shots: &[Shot],
        film: &[u8],
        notes: &[Note],
        failure: Option<&str>,
    ) -> io::Result<String> {
        let mut playback = Playback::new(film, self.width, self.height);
        let mut stills = Vec::with_capacity(shots.len());
        for (index, shot) in shots.iter().enumerate() {
            stills.push(Still {
                canvas: self.compose(
                    playback.frame(index)?,
                    shot.at_ms,
                    notes,
                    failure,
                    shot.caption.as_deref(),
                ),
                hold_ms: shot.hold_ms,
            });
        }
        if mp4(&stills, FPS, &self.dir.join("video.mp4"))? {
            return Ok("video.mp4".to_string());
        }
        fs::write(self.dir.join("video.gif"), gif(&stills))?;
        Ok("video.gif".to_string())
    }

    fn shots(&self, outcome: &Outcome<'_>) -> Vec<(usize, Shot)> {
        let frames = &self.frames_at;
        let Some(&start) = frames.first() else {
            return Vec::new();
        };
        let end = outcome
            .end_ms
            .max(frames.last().map_or(start, |&at| at) + 1000);
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
        let durations: Vec<(usize, u64, u64)> = (0..frames.len())
            .map(|i| {
                let from = frames[i];
                let until = frames.get(i + 1).map_or(end, |&at| at).max(from);
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
        let mut shots = Vec::new();
        let mut carried = 0.0;
        for (index, ms, real) in durations {
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
                .map(|c| c.name.clone());
            shots.push((
                index,
                Shot {
                    at_ms: frames[index],
                    hold_ms: shown as u32,
                    caption,
                },
            ));
        }
        shots
    }

    fn truth_at(&self, at_ms: u64) -> Option<&Truth> {
        let after = self.truth.partition_point(|t| t.at_ms <= at_ms);
        after.checked_sub(1).map(|i| &self.truth[i])
    }

    /// The device screen with a panel of time, scenario and recent events.
    fn compose(
        &self,
        pixels: &[u16],
        at_ms: u64,
        notes: &[Note],
        failure: Option<&str>,
        caption: Option<&str>,
    ) -> Canvas {
        let screen_w = self.width * SCALE;
        let screen_h = self.height * SCALE;
        let width = MARGIN * 3 + screen_w + PANEL_WIDTH;
        let height = MARGIN * 2 + screen_h + 40;
        let mut canvas = Canvas::new(width, height, BACKGROUND);
        canvas.fill_rect(MARGIN - 6, MARGIN - 6, screen_w + 12, screen_h + 12, BEZEL);
        canvas.blit565(MARGIN, MARGIN, self.width, self.height, pixels, SCALE);
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
        if let Some(truth) = self.truth_at(at_ms) {
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
        let recent: Vec<_> = notes
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
                canvas.text(MARGIN, status_y, 2, DIM, &footer(&self.device));
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
            "{{\n  \"name\": {},\n  \"device\": {},\n  \"status\": \"{}\",\n  \"failure\": {},\n  \"simulated_ms\": {},\n  \"frames\": {},\n  \"files\": [{}],\n  \"checkpoints\": [{}],\n  \"rides\": [{}]\n}}\n",
            json(&self.name),
            json(outcome.device),
            if failure.is_some() { "fail" } else { "pass" },
            failure.map_or("null".to_string(), json),
            outcome.end_ms,
            self.frames_at.len(),
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
pub(crate) fn wrap(
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

pub(crate) fn footer(device: &str) -> String {
    format!("VANA ON SIMULATED {}", device.to_uppercase())
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
    fn a_panic_location_names_no_machine() {
        let cases = [
            ("apps/vana/src/ride/log.rs", "apps/vana/src/ride/log.rs"),
            (
                "/home/rider/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/heapless-0.8.0/src/vec.rs",
                "heapless-0.8.0/src/vec.rs",
            ),
            (
                "/nix/store/x-cargo/git/checkouts/esp-hal-3c1d/4a5b6c7/esp-hal/src/lib.rs",
                "4a5b6c7/esp-hal/src/lib.rs",
            ),
            (
                "/rustc/0123456789abcdef/library/core/src/option.rs",
                "library/core/src/option.rs",
            ),
            ("/somewhere/else/build.rs", "build.rs"),
        ];
        for (file, shown) in cases {
            assert_eq!(portable(file), shown, "{file}");
        }
        capture_panics();
        let _ = std::panic::catch_unwind(|| panic!("planted"));
        let said = take_panic().expect("the hook kept the panic");
        assert!(
            said.starts_with("planted at tools/simulator/src/recording.rs:"),
            "{said}"
        );
    }

    #[test]
    fn quiet_stretches_merge_into_fewer_shots() {
        let frames: Vec<u64> = (0..6_000).map(|i| i * 1_000).collect();
        let outcome = Outcome {
            device: "c606",
            notes: &[note(0)],
            end_ms: 6_000_000,
            rides: Vec::new(),
            hashes: None,
        };
        let shown = recording(&frames).shots(&outcome);
        assert!(shown.len() < frames.len() / 2, "{} shots", shown.len());
        assert!(shown.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn names_and_text_are_safe_for_files_and_json() {
        assert_eq!(slug("Paired: HR + power!"), "paired-hr-power");
        assert_eq!(json("a \"b\"\n\\"), "\"a \\\"b\\\"\\n\\\\\"");
        assert_eq!(clock(65_400), "01:05.4");
        assert_eq!(clock(3_723_000), "1:02:03");
    }

    fn recording(frames: &[u64]) -> Recording {
        let mut recording = Recording::at(PathBuf::new(), "timing", "c606", 4, 2);
        for (i, &at) in frames.iter().enumerate() {
            recording.keep(at, &[i as u16; 8]);
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
            device: "c606",
            notes,
            end_ms,
            rides: Vec::new(),
            hashes: None,
        };
        let shots = recording.shots(&outcome);
        shots.iter().map(|(_, shot)| u64::from(shot.hold_ms)).sum()
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
