use super::Truth;
use crate::sim::{Note, NoteKind};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{self, BufReader, Read, Write as _};

pub(super) const FILE: &str = "frames.bin";
const VERSION: u8 = 2;

pub(super) struct Panels(fdeflate::Compressor<Vec<u8>>);

impl Panels {
    pub(super) fn new() -> Self {
        Self(fdeflate::Compressor::new(Vec::new()).expect("writing to memory cannot fail"))
    }

    pub(super) fn push(&mut self, before: &[u16], after: &[u16], width: usize) {
        self.0
            .write_data(&RowDeltas::encode(before, after, width))
            .expect("writing to memory cannot fail");
    }

    pub(super) fn finish(self) -> Vec<u8> {
        self.0.finish().expect("writing to memory cannot fail")
    }
}

pub(super) fn film(
    panels: &[u8],
    width: usize,
    height: usize,
    frames: &[usize],
) -> io::Result<Vec<u8>> {
    let mut playback = Playback::new(panels, width, height);
    let mut film = ZlibEncoder::new(Vec::new(), Compression::fast());
    let mut last = Vec::new();
    for &frame in frames {
        let pixels = playback.frame(frame)?;
        film.write_all(&RowDeltas::encode(&last, pixels, width))?;
        last.clear();
        last.extend_from_slice(pixels);
    }
    film.finish()
}

/// Each frame is stored as the rows that changed since the frame before, XORed
/// with it; the first frame is taken against a blank panel.
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

pub(super) struct Playback<'a> {
    stream: BufReader<ZlibDecoder<&'a [u8]>>,
    width: usize,
    pixels: Vec<u16>,
    played: usize,
}

impl<'a> Playback<'a> {
    pub(super) fn new(stream: &'a [u8], width: usize, height: usize) -> Self {
        Self {
            stream: BufReader::with_capacity(1 << 16, ZlibDecoder::new(stream)),
            width,
            pixels: vec![0; width * height],
            played: 0,
        }
    }

    pub(super) fn frame(&mut self, index: usize) -> io::Result<&[u16]> {
        if index + 1 < self.played {
            return Err(io::Error::other("tape playback only runs forward"));
        }
        while self.played <= index {
            RowDeltas::apply(&mut self.stream, &mut self.pixels, self.width)?;
            self.played += 1;
        }
        Ok(&self.pixels)
    }
}

pub(super) struct Shot {
    pub at_ms: u64,
    pub hold_ms: u32,
    pub caption: Option<String>,
}

pub(super) struct Header {
    pub name: String,
    pub device: String,
    pub width: usize,
    pub height: usize,
    pub failure: Option<String>,
    pub notes: Vec<Note>,
    pub truth: Vec<Truth>,
    pub shots: Vec<Shot>,
}

pub(super) fn write(header: &Header, film: &[u8]) -> Vec<u8> {
    let mut out = vec![VERSION];
    out.extend((header.width as u16).to_le_bytes());
    out.extend((header.height as u16).to_le_bytes());
    put_str(&mut out, &header.name);
    put_str(&mut out, &header.device);
    out.push(u8::from(header.failure.is_some()));
    put_str(&mut out, header.failure.as_deref().unwrap_or_default());
    out.extend((header.notes.len() as u32).to_le_bytes());
    for note in &header.notes {
        out.extend(note.at_ms.to_le_bytes());
        out.push(kind_byte(note.kind));
        put_str(&mut out, &note.text);
    }
    out.extend((header.truth.len() as u32).to_le_bytes());
    for truth in &header.truth {
        out.extend(truth.at_ms.to_le_bytes());
        for value in [truth.speed_kmh, truth.power_w, truth.heart_bpm] {
            out.extend(value.to_le_bytes());
        }
    }
    out.extend((header.shots.len() as u32).to_le_bytes());
    for shot in &header.shots {
        out.extend(shot.at_ms.to_le_bytes());
        out.extend(shot.hold_ms.to_le_bytes());
        put_str(&mut out, shot.caption.as_deref().unwrap_or_default());
    }
    out.extend(film);
    out
}

pub(super) fn read(bytes: &[u8]) -> io::Result<(Header, &[u8])> {
    let mut tape = Reader(bytes);
    if tape.take::<1>()? != [VERSION] {
        return Err(invalid(
            "frames.bin was written by another version of the simulator",
        ));
    }
    let width = u16::from_le_bytes(tape.take()?).into();
    let height = u16::from_le_bytes(tape.take()?).into();
    let name = tape.string()?;
    let device = tape.string()?;
    let failed = tape.take::<1>()? != [0];
    let failure = failed.then_some(tape.string()?);
    let notes = tape.list(|tape| {
        Ok(Note {
            at_ms: tape.u64()?,
            kind: kind(tape.take::<1>()?[0])?,
            text: tape.string()?,
        })
    })?;
    let truth = tape.list(|tape| {
        Ok(Truth {
            at_ms: tape.u64()?,
            speed_kmh: tape.f64()?,
            power_w: tape.f64()?,
            heart_bpm: tape.f64()?,
            elevation_m: None,
            nearest_vehicle_m: None,
        })
    })?;
    let shots = tape.list(|tape| {
        Ok(Shot {
            at_ms: tape.u64()?,
            hold_ms: tape.u32()?,
            caption: Some(tape.string()?).filter(|c| !c.is_empty()),
        })
    })?;
    if shots.is_empty() {
        return Err(invalid("frames.bin holds no frames"));
    }
    let header = Header {
        name,
        device,
        width,
        height,
        failure,
        notes,
        truth,
        shots,
    };
    Ok((header, tape.0))
}

fn kind_byte(kind: NoteKind) -> u8 {
    match kind {
        NoteKind::Input => 0,
        NoteKind::World => 1,
        NoteKind::Device => 2,
        NoteKind::Check => 3,
    }
}

fn kind(byte: u8) -> io::Result<NoteKind> {
    match byte {
        0 => Ok(NoteKind::Input),
        1 => Ok(NoteKind::World),
        2 => Ok(NoteKind::Device),
        3 => Ok(NoteKind::Check),
        _ => Err(invalid("frames.bin holds an unknown note kind")),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(text.as_bytes());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{Outcome, Recording, render_video};

    const PANELS: [[u16; 8]; 4] = [
        [1, 1, 1, 2, 3, 3, 0xffff, 0],
        [1, 1, 1, 2, 3, 3, 0xffff, 0],
        [1, 9, 1, 2, 3, 3, 0xffff, 7],
        [5, 5, 5, 5, 5, 5, 5, 5],
    ];

    fn stored() -> Vec<u8> {
        let mut panels = Panels::new();
        let mut last: &[u16] = &[];
        for panel in &PANELS {
            panels.push(last, panel, 4);
            last = panel;
        }
        panels.finish()
    }

    #[test]
    fn stored_panels_play_back_in_order() {
        let stored = stored();
        let mut playback = Playback::new(&stored, 4, 2);
        for (index, panel) in PANELS.iter().enumerate() {
            assert_eq!(playback.frame(index).unwrap(), panel);
        }
    }

    #[test]
    fn a_film_holds_only_the_frames_it_names() {
        let film = film(&stored(), 4, 2, &[1, 3]).unwrap();
        let mut playback = Playback::new(&film, 4, 2);
        assert_eq!(playback.frame(0).unwrap(), PANELS[1]);
        assert_eq!(playback.frame(1).unwrap(), PANELS[3]);
        assert!(playback.frame(2).is_err(), "the film ends after its frames");
    }

    #[test]
    fn a_video_rendered_after_the_run_matches_one_rendered_in_it() {
        let dir = std::env::temp_dir().join(format!("sim-tape-{}", std::process::id()));
        let mut recording = Recording::at(dir.clone(), "taped", "c606", 4, 2).with_video(true);
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
        let notes = [1_000, 4_000].map(|at_ms| Note {
            at_ms,
            kind: NoteKind::Input,
            text: "press".into(),
        });
        recording
            .write(Outcome {
                device: "c606",
                needs: &[],
                notes: &notes,
                end_ms: 6_000,
                rides: Vec::new(),
                hashes: None,
            })
            .unwrap();
        let video = ["video.mp4", "video.gif"]
            .into_iter()
            .find(|name| dir.join(name).exists())
            .expect("the run wrote a video");
        let in_run = std::fs::read(dir.join(video)).unwrap();
        std::fs::remove_file(dir.join(video)).unwrap();

        let rendered = render_video(&dir);
        let after = std::fs::read(dir.join(video));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(rendered.unwrap(), video);
        assert!(after.unwrap() == in_run, "the videos differ");
    }

    #[test]
    fn a_header_reads_back_as_written() {
        let notes = [
            NoteKind::Input,
            NoteKind::World,
            NoteKind::Device,
            NoteKind::Check,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| Note {
            at_ms: i as u64 * 100,
            kind,
            text: format!("note {i}"),
        })
        .collect();
        let header = Header {
            name: "taped".into(),
            device: "small".into(),
            width: 4,
            height: 2,
            failure: Some("boom".into()),
            notes,
            truth: vec![Truth {
                at_ms: 7,
                speed_kmh: 25.5,
                power_w: 180.0,
                heart_bpm: 140.0,
                elevation_m: None,
                nearest_vehicle_m: None,
            }],
            shots: vec![
                Shot {
                    at_ms: 0,
                    hold_ms: 50,
                    caption: None,
                },
                Shot {
                    at_ms: 300,
                    hold_ms: 1_500,
                    caption: Some("halfway".into()),
                },
            ],
        };
        let bytes = write(&header, b"film");
        let (back, film) = read(&bytes).unwrap();
        assert_eq!(film, b"film");
        assert_eq!(
            (
                back.name.as_str(),
                back.device.as_str(),
                back.width,
                back.height
            ),
            ("taped", "small", 4, 2)
        );
        assert_eq!(back.failure.as_deref(), Some("boom"));
        let notes = |h: &Header| {
            h.notes
                .iter()
                .map(|n| (n.at_ms, n.kind, n.text.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(notes(&back), notes(&header));
        let truth = |h: &Header| {
            h.truth
                .iter()
                .map(|t| (t.at_ms, t.speed_kmh, t.power_w, t.heart_bpm))
                .collect::<Vec<_>>()
        };
        assert_eq!(truth(&back), truth(&header));
        let shots = |h: &Header| {
            h.shots
                .iter()
                .map(|s| (s.at_ms, s.hold_ms, s.caption.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shots(&back), shots(&header));
        assert!(read(&bytes[..20]).is_err());
        let mut unknown = bytes.clone();
        let first_kind = 1 + 2 + 2 + (4 + 5) + 1 + (4 + 4) + 4 + 8;
        assert_eq!(unknown[first_kind], 0);
        unknown[first_kind] = 4;
        assert!(read(&unknown).is_err(), "an unknown note kind is refused");
    }
}
