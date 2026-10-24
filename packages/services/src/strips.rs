//! Which full-width strips of a frame a MIPI DCS panel already shows, so a
//! frame sends only the strips that changed (#167). One hash per strip of the
//! last frame sent stands in for its pixels.
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const COLUMN_WINDOW: u8 = 0x2a;
pub const ROW_WINDOW: u8 = 0x2b;
pub const WRITE: u8 = 0x2c;
pub const WRITE_CONTINUE: u8 = 0x3c;

/// Every this many frames all strips are sent, so a panel upset nothing
/// reports (or a hash collision) heals within a few seconds of motion.
pub const REFRESH_FRAMES: u32 = 128;

/// How to send one rendered strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Send {
    /// The panel already shows these pixels.
    Skip,
    /// Set the row window to `rows` (`ROW_WINDOW` parameters), then `WRITE`.
    Start { rows: [u8; 4] },
    /// `WRITE_CONTINUE` after the strip just above.
    Continue,
}

/// The last frame sent, as one hash per strip.
pub struct Shown<const STRIPS: usize> {
    hashes: [AtomicU32; STRIPS],
    known: AtomicBool,
    frames: AtomicU32,
}

impl<const STRIPS: usize> Default for Shown<STRIPS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const STRIPS: usize> Shown<STRIPS> {
    pub const fn new() -> Self {
        Self {
            hashes: [const { AtomicU32::new(0) }; STRIPS],
            known: AtomicBool::new(false),
            frames: AtomicU32::new(0),
        }
    }

    /// The panel may no longer show the last frame sent; the next frame
    /// sends every strip.
    pub fn forget(&self) {
        self.known.store(false, Ordering::Release);
    }

    /// Begin a frame of `STRIPS` strips of `strip_rows` rows. Strips must then
    /// be offered top to bottom, each once. A frame that fails part-way must
    /// call `forget`, since the panel holds an unknown mix of both frames.
    pub fn frame(&self, strip_rows: usize) -> Frame<'_, STRIPS> {
        let count = self.frames.load(Ordering::Relaxed);
        self.frames
            .store((count + 1) % REFRESH_FRAMES, Ordering::Relaxed);
        let known = self.known.swap(true, Ordering::AcqRel);
        Frame {
            shown: self,
            full: !known || count == 0,
            strip_rows,
            follows: None,
        }
    }
}

pub struct Frame<'a, const STRIPS: usize> {
    shown: &'a Shown<STRIPS>,
    full: bool,
    strip_rows: usize,
    /// The strip a `Continue` writes to: the one just below the last strip sent.
    follows: Option<usize>,
}

impl<const STRIPS: usize> Frame<'_, STRIPS> {
    /// Every strip is sent, and the column window must be set first.
    pub fn full(&self) -> bool {
        self.full
    }

    /// Decide how to send strip `strip`, given its pixels as the
    /// little-endian words the bus sends.
    pub fn send(&mut self, strip: usize, words: impl IntoIterator<Item = u32>) -> Send {
        let hash = hash(words);
        let previous = self.shown.hashes[strip].swap(hash, Ordering::Relaxed);
        if !self.full && previous == hash {
            return Send::Skip;
        }
        let send = if self.follows == Some(strip) {
            Send::Continue
        } else {
            let top = strip * self.strip_rows;
            Send::Start {
                rows: window(top, STRIPS * self.strip_rows - 1),
            }
        };
        self.follows = Some(strip + 1);
        send
    }
}

/// `COLUMN_WINDOW` or `ROW_WINDOW` parameters for `first..=last`.
pub fn window(first: usize, last: usize) -> [u8; 4] {
    [
        (first >> 8) as u8,
        first as u8,
        (last >> 8) as u8,
        last as u8,
    ]
}

/// Two interleaved FxHash lanes over 32-bit words, so the multiplies do not
/// wait on each other. Exact equality is all the caller needs.
pub fn hash(words: impl IntoIterator<Item = u32>) -> u32 {
    const K: u32 = 0x9e37_79b9;
    let mix = |hash: u32, word: u32| (hash.rotate_left(5) ^ word).wrapping_mul(K);
    let mut words = words.into_iter();
    let (mut a, mut b) = (0, K);
    while let Some(word) = words.next() {
        a = mix(a, word);
        if let Some(word) = words.next() {
            b = mix(b, word);
        }
    }
    mix(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Rng;
    use std::vec;
    use std::vec::Vec;

    const WIDTH: usize = 240;
    const HEIGHT: usize = 320;
    const ROWS: usize = 16;
    const STRIPS: usize = HEIGHT / ROWS;

    /// An ST7789's frame memory and address window, driven by the commands
    /// the display driver sends.
    struct Panel {
        pixels: Vec<u16>,
        columns: (usize, usize),
        rows: (usize, usize),
        at: (usize, usize),
        commands: usize,
        bytes: usize,
    }

    impl Panel {
        fn new(rng: &mut Rng) -> Self {
            Self {
                pixels: (0..WIDTH * HEIGHT).map(|_| rng.next() as u16).collect(),
                columns: (0, WIDTH - 1),
                rows: (0, HEIGHT - 1),
                at: (0, 0),
                commands: 0,
                bytes: 0,
            }
        }

        fn command(&mut self, command: u8, data: &[u8]) {
            self.commands += 1;
            let range = || {
                let [a, b, c, d] = data.try_into().expect("window parameters");
                (
                    usize::from(u16::from_be_bytes([a, b])),
                    usize::from(u16::from_be_bytes([c, d])),
                )
            };
            match command {
                COLUMN_WINDOW => self.columns = range(),
                ROW_WINDOW => self.rows = range(),
                WRITE | WRITE_CONTINUE => {
                    if command == WRITE {
                        self.at = (self.columns.0, self.rows.0);
                    }
                    self.bytes += data.len();
                    for &pixel in data.as_chunks::<2>().0 {
                        let (x, y) = self.at;
                        self.pixels[y * WIDTH + x] = u16::from_le_bytes(pixel);
                        self.at = if x < self.columns.1 {
                            (x + 1, y)
                        } else if y < self.rows.1 {
                            (self.columns.0, y + 1)
                        } else {
                            (self.columns.0, self.rows.0)
                        };
                    }
                }
                _ => panic!("unexpected command {command:#x}"),
            }
        }
    }

    /// The display driver's frame. `fail` stops it on a strip after that
    /// many of its bytes reached the panel.
    fn submit(
        shown: &Shown<STRIPS>,
        panel: &mut Panel,
        frame: &[u16],
        fail: Option<(usize, usize)>,
    ) {
        let mut plan = shown.frame(ROWS);
        if plan.full() {
            panel.command(COLUMN_WINDOW, &window(0, WIDTH - 1));
        }
        for (strip, pixels) in frame.as_chunks::<{ WIDTH * ROWS }>().0.iter().enumerate() {
            let bytes: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
            let words = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&w| u32::from_le_bytes(w));
            let send = plan.send(strip, words);
            let failed = fail.filter(|&(at, _)| at == strip);
            let bytes = &bytes[..failed.map_or(bytes.len(), |(_, sent)| sent)];
            match send {
                Send::Skip => {}
                Send::Start { rows } => {
                    panel.command(ROW_WINDOW, &rows);
                    panel.command(WRITE, bytes);
                }
                Send::Continue => panel.command(WRITE_CONTINUE, bytes),
            }
            if failed.is_some() {
                shown.forget();
                return;
            }
        }
    }

    /// Change a few rectangles, as gliding readings do, or now and then the
    /// whole screen, as a page change does.
    fn next(rng: &mut Rng, frame: &mut [u16]) {
        if rng.one_in(20) {
            let fill = rng.next() as u16;
            frame.iter_mut().for_each(|p| *p = fill);
        }
        for _ in 0..rng.below(4) {
            let (x, w) = (rng.below(WIDTH as u64) as usize, rng.range(1, 60) as usize);
            let (y, h) = (rng.below(HEIGHT as u64) as usize, rng.range(1, 40) as usize);
            let colour = rng.next() as u16;
            for row in y..(y + h).min(HEIGHT) {
                for x in x..(x + w).min(WIDTH) {
                    frame[row * WIDTH + x] = colour;
                }
            }
        }
    }

    #[test]
    fn partial_frames_leave_the_panel_showing_each_full_frame() {
        for seed in 0..12 {
            let mut rng = Rng::new(seed);
            let shown = Shown::<STRIPS>::new();
            let mut panel = Panel::new(&mut rng);
            let mut frame = vec![0u16; WIDTH * HEIGHT];
            for step in 0..300 {
                next(&mut rng, &mut frame);
                if rng.one_in(40) {
                    let strip = rng.below(STRIPS as u64) as usize;
                    let sent = 2 * rng.below((WIDTH * ROWS) as u64) as usize;
                    submit(&shown, &mut panel, &frame, Some((strip, sent)));
                    next(&mut rng, &mut frame);
                }
                if rng.one_in(50) {
                    panel = Panel::new(&mut rng);
                    shown.forget();
                }
                submit(&shown, &mut panel, &frame, None);
                assert!(panel.pixels == frame, "seed {seed} frame {step}");
            }
        }
    }

    #[test]
    fn an_unchanged_frame_sends_nothing_and_one_changed_row_sends_its_strip() {
        let mut rng = Rng::new(7);
        let shown = Shown::<STRIPS>::new();
        let mut panel = Panel::new(&mut rng);
        let mut frame = vec![0x1234u16; WIDTH * HEIGHT];
        submit(&shown, &mut panel, &frame, None);
        assert_eq!(panel.bytes, WIDTH * HEIGHT * 2);
        assert_eq!(panel.commands, 2 + STRIPS);

        let (commands, bytes) = (panel.commands, panel.bytes);
        submit(&shown, &mut panel, &frame, None);
        assert_eq!((panel.commands, panel.bytes), (commands, bytes));

        frame[100 * WIDTH + 7] = 0xffff;
        frame[101 * WIDTH + 7] = 0xffff;
        submit(&shown, &mut panel, &frame, None);
        assert_eq!(panel.commands - commands, 2);
        assert_eq!(panel.bytes - bytes, WIDTH * ROWS * 2);
        assert!(panel.pixels == frame);
    }

    #[test]
    fn every_strip_is_sent_again_after_the_refresh_interval() {
        let mut rng = Rng::new(3);
        let shown = Shown::<STRIPS>::new();
        let mut panel = Panel::new(&mut rng);
        let frame = vec![0u16; WIDTH * HEIGHT];
        let mut full = Vec::new();
        for index in 0..2 * REFRESH_FRAMES + 1 {
            let bytes = panel.bytes;
            submit(&shown, &mut panel, &frame, None);
            if panel.bytes > bytes {
                full.push(index);
            }
        }
        assert_eq!(full, [0, REFRESH_FRAMES, 2 * REFRESH_FRAMES]);
    }
}
