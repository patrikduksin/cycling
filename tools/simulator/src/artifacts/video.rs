//! Animated GIF and MP4 encoding of timed stills.

use super::canvas::Canvas;
use std::collections::{HashMap, hash_map::Entry};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// One video frame and how long it stays on screen.
pub struct Still {
    pub canvas: Canvas,
    pub hold_ms: u32,
}

type Palette = (Vec<[u8; 3]>, HashMap<[u8; 3], u8>);

/// Looping GIF with a global palette when the stills use at most 256 colours.
pub fn gif(stills: &[Still]) -> Vec<u8> {
    let (width, height) = stills
        .first()
        .map_or((1, 1), |s| (s.canvas.width as u16, s.canvas.height as u16));
    let palette = global_palette(stills);
    let global: Vec<u8> = palette
        .as_ref()
        .map(|(colors, _)| colors.concat())
        .unwrap_or_default();
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, width, height, &global)
            .expect("writing into a Vec cannot fail");
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .expect("writing into a Vec cannot fail");
        for still in stills {
            let mut frame = match &palette {
                Some((_, index)) => {
                    let indices: Vec<u8> = still
                        .canvas
                        .rgb
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|c| index[c])
                        .collect();
                    gif::Frame {
                        width,
                        height,
                        buffer: indices.into(),
                        ..gif::Frame::default()
                    }
                }
                None => gif::Frame::from_rgb_speed(width, height, &still.canvas.rgb, 10),
            };
            frame.delay = (still.hold_ms.saturating_add(5) / 10).clamp(2, u16::MAX as u32) as u16;
            encoder
                .write_frame(&frame)
                .expect("writing into a Vec cannot fail");
        }
    }
    out
}

fn global_palette(stills: &[Still]) -> Option<Palette> {
    let mut colors = Vec::new();
    let mut index = HashMap::new();
    for still in stills {
        for &color in still.canvas.rgb.as_chunks::<3>().0 {
            if let Entry::Vacant(slot) = index.entry(color) {
                if colors.len() == 256 {
                    return None;
                }
                slot.insert(colors.len() as u8);
                colors.push(color);
            }
        }
    }
    Some((colors, index))
}

/// Writes an H.264 MP4 via `ffmpeg`; `Ok(false)` means ffmpeg is not installed.
pub fn mp4(stills: &[Still], fps: u32, path: &Path) -> io::Result<bool> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidInput, message.to_owned());
    let first = stills.first().ok_or_else(|| invalid("no stills"))?;
    let (width, height) = (first.canvas.width, first.canvas.height);
    if fps == 0
        || stills
            .iter()
            .any(|s| (s.canvas.width, s.canvas.height) != (width, height))
    {
        return Err(invalid(
            "fps must be positive and stills must share one size",
        ));
    }
    let mut child = match Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
        ])
        .args([
            "-s",
            &format!("{width}x{height}"),
            "-r",
            &fps.to_string(),
            "-i",
            "-",
        ])
        .args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"])
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "20",
            "-tune",
            "animation",
        ])
        .args(["-movflags", "+faststart"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let output = std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut elapsed_ms = 0u64;
            let mut written = 0u64;
            for still in stills {
                elapsed_ms += u64::from(still.hold_ms);
                let target = (elapsed_ms * u64::from(fps) + 500) / 1000;
                for _ in 0..target.saturating_sub(written).max(1) {
                    if stdin.write_all(&still.canvas.rgb).is_err() {
                        return;
                    }
                    written += 1;
                }
            }
        });
        child.wait_with_output()
    })?;
    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::other(format!(
            "ffmpeg failed ({}): {}",
            output.status,
            stderr.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn still(color: [u8; 3], hold_ms: u32) -> Still {
        Still {
            canvas: Canvas::new(16, 10, color),
            hold_ms,
        }
    }

    #[test]
    fn gif_keeps_frames_and_delays() {
        let stills = [
            still([255, 0, 0], 500),
            still([0, 0, 255], 1),
            still([0, 255, 0], 120),
        ];
        let data = gif(&stills);
        let mut decoder = gif::DecodeOptions::new()
            .read_info(std::io::Cursor::new(data))
            .unwrap();
        let mut delays = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            delays.push(frame.delay);
        }
        assert_eq!(delays, [50, 2, 12]);
    }

    #[test]
    fn gif_falls_back_beyond_256_colours() {
        let mut canvas = Canvas::new(32, 32, [0, 0, 0]);
        for i in 0..1024usize {
            canvas.set(
                i % 32,
                i / 32,
                [(i % 256) as u8, (i / 4) as u8, (i / 7) as u8],
            );
        }
        let data = gif(&[Still {
            canvas,
            hold_ms: 100,
        }]);
        assert!(data.starts_with(b"GIF89a"));
    }

    #[test]
    fn mp4_writes_an_ftyp_box_when_ffmpeg_exists() {
        let path =
            std::env::temp_dir().join(format!("simulator-artifacts-{}.mp4", std::process::id()));
        let stills = [still([255, 0, 0], 200), still([0, 0, 255], 200)];
        if mp4(&stills, 10, &path).unwrap() {
            let data = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            assert_eq!(&data[4..8], b"ftyp");
        }
    }
}
