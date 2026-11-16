//! Scenario recordings and the artifacts a reviewer reads: a captioned video,
//! checkpoint screenshots, a chart of the world, recorded rides and a report.
//!
//! Recording is enabled by `SIM_ARTIFACTS=<directory>`; each scenario writes
//! `<directory>/<scenario>/` when its `Sim` is dropped, including on failure.
//! The video is written only with `SIM_VIDEO=1`, but every scenario stores the
//! frames it is made from in `frames.bin`, so [`render_video`] can make it
//! later without running the scenario again.
use crate::artifacts::{Canvas, Series, Still, chart_png, chart_svg, gif, mp4};
use crate::sim::{Note, NoteKind};
use crate::world::World;
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufReader, Read, Write as _};
use std::path::{Path, PathBuf};

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
                .map(|l| format!(" at {}:{}", l.file(), l.line()))
                .unwrap_or_default();
            PANIC.with(|p| *p.borrow_mut() = Some(format!("{message}{location}")));
            default(info);
        }));
    });
}

/// The message of this thread's last panic, once.
pub fn take_panic() -> Option<String> {
    PANIC.with(|p| p.borrow_mut().take())
}

const TAPE: &str = "frames.bin";
const TAPE_VERSION: u8 = 1;

/// Every frame of every scenario is encoded here, so this uses fdeflate's fast
/// fixed-table encoder, built for mostly-zero rows like these deltas. The film
/// stored for the video uses flate2's fast level: the default level makes it
/// 28% smaller but costs about 11 more CPU-seconds per run of every scenario.
struct Panels {
    stream: Option<fdeflate::Compressor<Vec<u8>>>,
    finished: Vec<u8>,
}

impl Panels {
    fn new() -> Self {
        Self {
            stream: Some(
                fdeflate::Compressor::new(Vec::new()).expect("writing to memory cannot fail"),
            ),
            finished: Vec::new(),
        }
    }

    fn push(&mut self, before: &[u16], after: &[u16], width: usize) {
        if let Some(stream) = &mut self.stream {
            stream
                .write_data(&RowDeltas::encode(before, after, width))
                .expect("writing to memory cannot fail");
        }
    }

    fn finish(&mut self) -> &[u8] {
        if let Some(stream) = self.stream.take() {
            self.finished = stream.finish().expect("writing to memory cannot fail");
        }
        &self.finished
    }
}

struct Shot {
    /// Its index among the frames the recording kept.
    frame: usize,
    at_ms: u64,
    hold_ms: u32,
    caption: Option<String>,
}

/// A panel as the rows that differ from the panel before it (all zeros at the
/// start), each as its differing span XORed with what was there: a row count,
/// then per row its y, x0, x1 and the span.
struct RowDeltas;

impl RowDeltas {
    fn encode(before: &[u16], after: &[u16], width: usize) -> Vec<u8> {
        let blank;
        let before = if before.len() == after.len() {
            before
        } else {
            blank = vec![0; after.len()];
            &blank
        };
        let mut bytes = vec![0, 0];
        let mut rows: u16 = 0;
        for (y, (a, b)) in before.chunks(width).zip(after.chunks(width)).enumerate() {
            if a == b {
                continue;
            }
            let differs = |(p, q): (&u16, &u16)| p != q;
            let x0 = a.iter().zip(b).position(differs).unwrap_or(0);
            let x1 = width
                - a.iter()
                    .rev()
                    .zip(b.iter().rev())
                    .position(differs)
                    .unwrap_or(0);
            for edge in [y, x0, x1] {
                bytes.extend_from_slice(&(edge as u16).to_le_bytes());
            }
            for (p, q) in a[x0..x1].iter().zip(&b[x0..x1]) {
                bytes.extend_from_slice(&(p ^ q).to_le_bytes());
            }
            rows += 1;
        }
        bytes[..2].copy_from_slice(&rows.to_le_bytes());
        bytes
    }

    fn apply(stream: &mut impl Read, pixels: &mut [u16], width: usize) -> io::Result<()> {
        let mut count = [0; 2];
        stream.read_exact(&mut count)?;
        let mut span = Vec::new();
        for _ in 0..u16::from_le_bytes(count) {
            let mut edges = [0; 6];
            stream.read_exact(&mut edges)?;
            let edge = |i: usize| usize::from(u16::from_le_bytes([edges[i], edges[i + 1]]));
            let (y, x0, x1) = (edge(0), edge(2), edge(4));
            if x0 > x1 || x1 > width || (y + 1) * width > pixels.len() {
                return Err(invalid("a frame reaches outside the panel"));
            }
            span.resize((x1 - x0) * 2, 0);
            stream.read_exact(&mut span)?;
            let line = &mut pixels[y * width + x0..y * width + x1];
            for (pixel, bytes) in line.iter_mut().zip(span.as_chunks::<2>().0) {
                *pixel ^= u16::from_le_bytes(*bytes);
            }
        }
        Ok(())
    }
}

struct Playback<'a> {
    stream: BufReader<ZlibDecoder<&'a [u8]>>,
    width: usize,
    pixels: Vec<u16>,
    played: usize,
}

impl<'a> Playback<'a> {
    fn new(stream: &'a [u8], width: usize, height: usize) -> Self {
        Self {
            stream: BufReader::with_capacity(1 << 16, ZlibDecoder::new(stream)),
            width,
            pixels: vec![0; width * height],
            played: 0,
        }
    }

    /// Frames play forward only.
    fn frame(&mut self, index: usize) -> io::Result<&[u16]> {
        while self.played <= index {
            RowDeltas::apply(&mut self.stream, &mut self.pixels, self.width)?;
            self.played += 1;
        }
        Ok(&self.pixels)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

const NOTE_KINDS: [NoteKind; 4] = [
    NoteKind::Input,
    NoteKind::World,
    NoteKind::Device,
    NoteKind::Check,
];

fn put_str(out: &mut Vec<u8>, text: &str) {
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(text.as_bytes());
}

struct Tape<'a>(&'a [u8]);

impl<'a> Tape<'a> {
    fn take<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let (head, rest) = self
            .0
            .split_first_chunk()
            .ok_or_else(|| invalid("frames.bin ends early"))?;
        self.0 = rest;
        Ok(*head)
    }

    fn u32(&mut self) -> io::Result<u32> {
        self.take().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> io::Result<u64> {
        self.take().map(u64::from_le_bytes)
    }

    fn f64(&mut self) -> io::Result<f64> {
        self.take().map(f64::from_le_bytes)
    }

    fn string(&mut self) -> io::Result<String> {
        let len = self.u32()? as usize;
        if len > self.0.len() {
            return Err(invalid("frames.bin ends early"));
        }
        let (text, rest) = self.0.split_at(len);
        self.0 = rest;
        String::from_utf8(text.to_vec()).map_err(|_| invalid("frames.bin holds invalid text"))
    }

    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> io::Result<T>) -> io::Result<Vec<T>> {
        (0..self.u32()?).map(|_| item(self)).collect()
    }
}

/// Render the video of the scenario recorded in `dir` from the shots it
/// stored, as if it had run with `SIM_VIDEO=1`; returns the file written.
pub fn render_video(dir: &Path) -> io::Result<String> {
    let bytes = fs::read(dir.join(TAPE))?;
    let mut tape = Tape(&bytes);
    if tape.take::<1>()? != [TAPE_VERSION] {
        return Err(invalid(
            "frames.bin was written by another version of the simulator",
        ));
    }
    let width = u16::from_le_bytes(tape.take()?).into();
    let height = u16::from_le_bytes(tape.take()?).into();
    let mut recording = Recording::at(dir.to_path_buf(), &tape.string()?, width, height);
    let failed = tape.take::<1>()? != [0];
    let failure = tape.string()?;
    let notes = tape.list(|tape| {
        let at_ms = tape.u64()?;
        let [kind] = tape.take()?;
        Ok(Note {
            at_ms,
            kind: *NOTE_KINDS
                .get(usize::from(kind))
                .ok_or_else(|| invalid("frames.bin holds an unknown note kind"))?,
            text: tape.string()?,
        })
    })?;
    recording.truth = tape.list(|tape| {
        Ok(Truth {
            at_ms: tape.u64()?,
            speed_kmh: tape.f64()?,
            power_w: tape.f64()?,
            heart_bpm: tape.f64()?,
            elevation_m: None,
            nearest_vehicle_m: None,
        })
    })?;
    let mut frame = 0;
    let shots = tape.list(|tape| {
        frame += 1;
        Ok(Shot {
            frame: frame - 1,
            at_ms: tape.u64()?,
            hold_ms: tape.u32()?,
            caption: Some(tape.string()?).filter(|c| !c.is_empty()),
        })
    })?;
    let failure = failed.then_some(failure.as_str());
    recording
        .video(&shots, tape.0, &notes, failure)?
        .ok_or_else(|| invalid("frames.bin holds no frames"))
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
        let thread = std::thread::current();
        let name = thread
            .name()
            .unwrap_or("scenario")
            .rsplit("::")
            .next()
            .unwrap_or("scenario");
        Self::named(name, width, height)
    }

    /// A recording called `name`, when artifacts are requested.
    pub fn named(name: &str, width: usize, height: usize) -> Option<Self> {
        let root = std::env::var_os("SIM_ARTIFACTS")?;
        let video = std::env::var_os("SIM_VIDEO").is_some_and(|v| v == "1");
        let name = claim(name.to_string());
        Some(Self::at(Path::new(&root).join(&name), &name, width, height).with_video(video))
    }

    pub fn at(dir: PathBuf, name: &str, width: usize, height: usize) -> Self {
        capture_panics();
        PANIC.with(|p| p.borrow_mut().take());
        Self {
            dir,
            name: name.to_string(),
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
    pub fn write(&mut self, outcome: Outcome<'_>) -> io::Result<()> {
        let failure = std::thread::panicking()
            .then(|| PANIC.with(|p| p.borrow_mut().take()).unwrap_or_default());
        replace_dir(&self.dir)?;
        let mut files = Vec::new();

        let shots = self.shots(&outcome);
        if !shots.is_empty() {
            let film = self.film(&shots)?;
            if self.video
                && let Some(video) = self.video(&shots, &film, outcome.notes, failure.as_deref())?
            {
                files.push(video);
            }
            fs::write(
                self.dir.join(TAPE),
                self.tape(&shots, &film, outcome.notes, failure.as_deref()),
            )?;
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
        fs::write(self.dir.join("timeline.txt"), timeline(outcome.notes))?;
        fs::write(
            self.dir.join("result.json"),
            self.result(&outcome, failure.as_deref(), &files),
        )?;
        Ok(())
    }

    fn tape(&self, shots: &[Shot], film: &[u8], notes: &[Note], failure: Option<&str>) -> Vec<u8> {
        let mut out = vec![TAPE_VERSION];
        out.extend((self.width as u16).to_le_bytes());
        out.extend((self.height as u16).to_le_bytes());
        put_str(&mut out, &self.name);
        out.push(u8::from(failure.is_some()));
        put_str(&mut out, failure.unwrap_or_default());
        out.extend((notes.len() as u32).to_le_bytes());
        for note in notes {
            out.extend(note.at_ms.to_le_bytes());
            out.push(
                NOTE_KINDS
                    .iter()
                    .position(|&kind| kind == note.kind)
                    .unwrap_or(0) as u8,
            );
            put_str(&mut out, &note.text);
        }
        let mut shown: Vec<&Truth> = shots
            .iter()
            .filter_map(|shot| self.truth_at(shot.at_ms))
            .collect();
        shown.dedup_by_key(|t| t.at_ms);
        out.extend((shown.len() as u32).to_le_bytes());
        for truth in shown {
            out.extend(truth.at_ms.to_le_bytes());
            for value in [truth.speed_kmh, truth.power_w, truth.heart_bpm] {
                out.extend(value.to_le_bytes());
            }
        }
        out.extend((shots.len() as u32).to_le_bytes());
        for shot in shots {
            out.extend(shot.at_ms.to_le_bytes());
            out.extend(shot.hold_ms.to_le_bytes());
            put_str(&mut out, shot.caption.as_deref().unwrap_or_default());
        }
        out.extend(film);
        out
    }

    fn film(&mut self, shots: &[Shot]) -> io::Result<Vec<u8>> {
        let (width, height) = (self.width, self.height);
        let mut playback = Playback::new(self.panels.finish(), width, height);
        let mut film = ZlibEncoder::new(Vec::new(), Compression::fast());
        let mut last = Vec::new();
        for shot in shots {
            let pixels = playback.frame(shot.frame)?;
            film.write_all(&RowDeltas::encode(&last, pixels, width))?;
            last.clear();
            last.extend_from_slice(pixels);
        }
        film.finish()
    }

    fn video(
        &self,
        shots: &[Shot],
        film: &[u8],
        notes: &[Note],
        failure: Option<&str>,
    ) -> io::Result<Option<String>> {
        if shots.is_empty() {
            return Ok(None);
        }
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
            return Ok(Some("video.mp4".to_string()));
        }
        fs::write(self.dir.join("video.gif"), gif(&stills))?;
        Ok(Some("video.gif".to_string()))
    }

    /// Every distinct frame, held for its compressed duration.
    fn shots(&self, outcome: &Outcome<'_>) -> Vec<Shot> {
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
            shots.push(Shot {
                frame: index,
                at_ms: frames[index],
                hold_ms: shown as u32,
                caption,
            });
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
    fn stored_panels_play_back_in_order() {
        let panels: [[u16; 8]; 4] = [
            [1, 1, 1, 2, 3, 3, 0xffff, 0],
            [1, 1, 1, 2, 3, 3, 0xffff, 0],
            [1, 9, 1, 2, 3, 3, 0xffff, 7],
            [5, 5, 5, 5, 5, 5, 5, 5],
        ];
        let mut stored = Panels::new();
        let mut last: &[u16] = &[];
        for panel in &panels {
            stored.push(last, panel, 4);
            last = panel;
        }
        let mut playback = Playback::new(stored.finish(), 4, 2);
        for (index, panel) in panels.iter().enumerate() {
            assert_eq!(playback.frame(index).unwrap(), panel);
        }
    }

    #[test]
    fn the_film_keeps_only_the_panels_its_shots_show() {
        let frames: Vec<u64> = (0..6_000).map(|i| i * 1_000).collect();
        let mut recording = recording(&frames);
        let outcome = Outcome {
            notes: &[note(0)],
            end_ms: 6_000_000,
            rides: Vec::new(),
        };
        let shown = recording.shots(&outcome);
        assert!(shown.len() < frames.len() / 2, "{} shots", shown.len());
        let film = recording.film(&shown).unwrap();
        let mut playback = Playback::new(&film, 4, 2);
        for (index, shot) in shown.iter().enumerate() {
            assert_eq!(playback.frame(index).unwrap(), [shot.frame as u16; 8]);
        }
    }

    #[test]
    fn a_video_rendered_after_the_run_matches_one_rendered_in_it() {
        let dir = std::env::temp_dir().join(format!("sim-tape-{}", std::process::id()));
        let mut recording = Recording::at(dir.clone(), "taped", 4, 2).with_video(true);
        recording.truth.push(Truth {
            at_ms: 0,
            speed_kmh: 25.0,
            power_w: 180.0,
            heart_bpm: 140.0,
            elevation_m: None,
            nearest_vehicle_m: None,
        });
        for at in 0..50 {
            let mut pixels = [7; 8];
            pixels[(at / 10) as usize] = at as u16;
            recording.observe(at * 100, at as u32, &pixels, None);
        }
        recording.checkpoint("halfway", 2_500, &[3; 8], 1);
        let notes = [note(1_000), note(4_000)];
        recording
            .write(Outcome {
                notes: &notes,
                end_ms: 6_000,
                rides: Vec::new(),
            })
            .unwrap();
        let video = ["video.mp4", "video.gif"]
            .into_iter()
            .find(|name| dir.join(name).exists())
            .expect("the run wrote a video");
        let in_run = fs::read(dir.join(video)).unwrap();
        fs::remove_file(dir.join(video)).unwrap();

        let rendered = render_video(&dir);
        let after = fs::read(dir.join(video));
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(rendered.unwrap(), video);
        assert!(after.unwrap() == in_run, "the videos differ");
    }

    #[test]
    fn names_and_text_are_safe_for_files_and_json() {
        assert_eq!(slug("Paired: HR + power!"), "paired-hr-power");
        assert_eq!(json("a \"b\"\n\\"), "\"a \\\"b\\\"\\n\\\\\"");
        assert_eq!(clock(65_400), "01:05.4");
        assert_eq!(clock(3_723_000), "1:02:03");
    }

    fn recording(frames: &[u64]) -> Recording {
        let mut recording = Recording::at(PathBuf::new(), "timing", 4, 2);
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
            notes,
            end_ms,
            rides: Vec::new(),
        };
        let shots = recording.shots(&outcome);
        shots.iter().map(|shot| u64::from(shot.hold_ms)).sum()
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
