//! Recovers text from an RGB565 frame drawn with the shared 5x7 bitmap font
//! or the anti-aliased faces of `firmware_shell::rendering::font`.
use firmware_shell::rendering::font::{Face, Faces, SCALE_1, blend};
use firmware_shell::rendering::row::{Look, Shown};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Mirrors the private glyph table in `firmware_shell::rendering::text`; a unit
/// test compares both so drift is caught. Rows use five low bits, leftmost dot
/// in bit four.
const GLYPHS: [(u8, [u8; 7]); 44] = [
    (b':', [0, 6, 6, 0, 6, 6, 0]),
    (b'/', [1, 1, 2, 4, 8, 16, 16]),
    (b'+', [0, 4, 4, 31, 4, 4, 0]),
    (b'%', [25, 26, 2, 4, 8, 11, 19]),
    (b'Z', [31, 1, 2, 4, 8, 16, 31]),
    (b'J', [7, 2, 2, 2, 18, 18, 12]),
    (b',', [0, 0, 0, 0, 6, 6, 4]),
    (b'.', [0, 0, 0, 0, 0, 6, 6]),
    (b'Q', [14, 17, 17, 17, 21, 18, 13]),
    (b'B', [30, 17, 17, 30, 17, 17, 30]),
    (b'Y', [17, 17, 10, 4, 4, 4, 4]),
    (b'>', [16, 8, 4, 2, 4, 8, 16]),
    (b'-', [0, 0, 0, 31, 0, 0, 0]),
    (b'M', [17, 27, 21, 21, 17, 17, 17]),
    (b'U', [17, 17, 17, 17, 17, 17, 14]),
    (b'A', [14, 17, 17, 31, 17, 17, 17]),
    (b'C', [14, 17, 16, 16, 16, 17, 14]),
    (b'D', [30, 17, 17, 17, 17, 17, 30]),
    (b'E', [31, 16, 16, 30, 16, 16, 31]),
    (b'F', [31, 16, 16, 30, 16, 16, 16]),
    (b'H', [17, 17, 17, 31, 17, 17, 17]),
    (b'G', [14, 17, 16, 23, 17, 17, 15]),
    (b'I', [31, 4, 4, 4, 4, 4, 31]),
    (b'K', [17, 18, 20, 24, 20, 18, 17]),
    (b'L', [16, 16, 16, 16, 16, 16, 31]),
    (b'N', [17, 25, 25, 21, 19, 19, 17]),
    (b'O', [14, 17, 17, 17, 17, 17, 14]),
    (b'P', [30, 17, 17, 30, 16, 16, 16]),
    (b'R', [30, 17, 17, 30, 20, 18, 17]),
    (b'S', [15, 16, 16, 14, 1, 1, 30]),
    (b'T', [31, 4, 4, 4, 4, 4, 4]),
    (b'V', [17, 17, 17, 17, 17, 10, 4]),
    (b'W', [17, 17, 17, 21, 21, 21, 10]),
    (b'X', [17, 17, 10, 4, 10, 17, 17]),
    (b'0', [14, 17, 19, 21, 25, 17, 14]),
    (b'1', [4, 12, 4, 4, 4, 4, 14]),
    (b'2', [14, 17, 1, 2, 4, 8, 31]),
    (b'3', [30, 1, 1, 14, 1, 1, 30]),
    (b'4', [2, 6, 10, 18, 31, 2, 2]),
    (b'5', [31, 16, 16, 30, 1, 1, 30]),
    (b'6', [14, 16, 16, 30, 17, 17, 14]),
    (b'7', [31, 1, 2, 4, 8, 8, 8]),
    (b'8', [14, 17, 17, 14, 17, 17, 14]),
    (b'9', [14, 17, 17, 15, 1, 1, 14]),
];

const MAX_SCALE: usize = 8;

const KEYS: [usize; 16] = [0, 7, 5, 28, 17, 18, 14, 34, 20, 33, 1, 2, 3, 4, 6, 8];
const PROBES: usize = 5;

struct Lookup {
    masks: Vec<(u64, u8)>,
    keys: Vec<u64>,
    first_dots: Vec<(usize, usize)>,
}

impl Lookup {
    fn get() -> &'static Self {
        static LOOKUP: OnceLock<Lookup> = OnceLock::new();
        LOOKUP.get_or_init(|| {
            let mut masks: Vec<(u64, u8)> = GLYPHS
                .iter()
                .map(|&(character, rows)| (mask(&rows), character))
                .collect();
            masks.sort_unstable();
            let mut keys = vec![0_u64; (1 << KEYS.len()) / 64];
            for &(bits, _) in &masks {
                let key = KEYS
                    .iter()
                    .enumerate()
                    .fold(0, |key, (i, &index)| key | (bits >> index & 1) << i);
                keys[key as usize / 64] |= 1 << (key % 64);
            }
            let mut first_dots: Vec<_> = masks
                .iter()
                .map(|&(bits, _)| bits.trailing_zeros() as usize)
                .map(|dot| (dot / 5, dot % 5))
                .collect();
            first_dots.sort_unstable();
            first_dots.dedup();
            Self {
                masks,
                keys,
                first_dots,
            }
        })
    }

    fn keyed(&self, key: usize) -> bool {
        self.keys[key / 64] >> (key % 64) & 1 == 1
    }

    fn character(&self, bits: u64) -> Option<u8> {
        let found = self.masks.binary_search_by_key(&bits, |&(mask, _)| mask);
        found.ok().map(|found| self.masks[found].1)
    }
}

pub struct Frame<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a [u16],
    pub look: Look,
    /// The faces of the panel's scale step, which its smooth text is read in.
    pub faces: &'static Faces,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRun {
    pub text: String,
    pub x: usize,
    pub y: usize,
    /// The 5x7 scale, or for anti-aliased text the 5x7 scale of about the
    /// same capital height.
    pub scale: usize,
    pub color: u16,
    /// Drawn in an anti-aliased face rather than the 5x7 font.
    pub smooth: bool,
}

#[derive(Clone, Copy)]
struct Glyph {
    character: u8,
    x: usize,
    y: usize,
    /// 5x7 scale, or `FACE_SCALE` plus the face's index.
    scale: usize,
    color: u16,
    ink: u32,
    /// Pen advance to the next glyph.
    advance: usize,
}

/// Anti-aliased glyphs group apart from 5x7 scales.
const FACE_SCALE: usize = 100;

/// A face's glyph by its fully covered pixels, relative to the first of them
/// in raster order. Full coverage is drawn exactly in the text colour.
struct Template {
    character: u8,
    face: usize,
    /// The anchor pixel within the glyph box.
    anchor: (usize, usize),
    /// Glyph box offset from the pen position and its size.
    offset: (i32, i32),
    size: (usize, usize),
    solid: Vec<bool>,
    /// The solid box pixels, so a mismatch is found in a few reads.
    solids: Vec<usize>,
    /// Coverage 0..=15 of every box pixel.
    coverage: Vec<u8>,
    /// Box pixels the glyph only partly covers, and those it leaves clear.
    edges: Vec<usize>,
    clear: Vec<usize>,
    ink: u32,
}

struct Templates {
    exact: HashMap<(usize, usize), Vec<Template>>,
    /// Keys by `down`, longest `right` first: a run that continues into the
    /// next glyph, as in "TT", is longer than its template's own.
    by_down: HashMap<usize, Vec<(usize, usize)>>,
}

fn templates(faces: &'static Faces) -> &'static Templates {
    static SETS: Mutex<Vec<(&'static Faces, &'static Templates)>> = Mutex::new(Vec::new());
    let mut sets = SETS.lock().expect("template sets");
    if let Some(&(_, built)) = sets.iter().find(|(set, _)| *set == faces) {
        return built;
    }
    let built = Box::leak(Box::new(build_templates(faces)));
    sets.push((faces, built));
    built
}

fn build_templates(faces: &Faces) -> Templates {
    let mut map: HashMap<(usize, usize), Vec<Template>> = HashMap::new();
    for (face_index, face) in faces.0.iter().enumerate() {
        for character in 0x21..=0x5f_u8 {
            let glyph = face.glyph(character);
            let (w, h) = (usize::from(glyph.w), usize::from(glyph.h));
            if w == 0 {
                continue;
            }
            let at = |x: usize, y: usize| {
                face.coverage(
                    character,
                    x as i32 + i32::from(glyph.x),
                    y as i32 + i32::from(glyph.y),
                )
            };
            let coverage: Vec<u8> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| at(x, y))
                .collect();
            let solid: Vec<bool> = coverage.iter().map(|&c| c == 15).collect();
            let Some(first) = solid.iter().position(|&s| s) else {
                continue;
            };
            let anchor = (first % w, first / w);
            let right = (anchor.0..w)
                .take_while(|&x| solid[anchor.1 * w + x])
                .count();
            let down = (anchor.1..h)
                .take_while(|&y| solid[y * w + anchor.0])
                .count();
            let solids: Vec<usize> = (0..w * h).filter(|&i| solid[i]).collect();
            let ink = solids.len() as u32;
            let edges = (0..w * h)
                .filter(|&i| (1..15).contains(&coverage[i]))
                .collect();
            let clear = (0..w * h).filter(|&i| coverage[i] == 0).collect();
            map.entry((right, down)).or_default().push(Template {
                character,
                face: face_index,
                anchor,
                offset: (i32::from(glyph.x), i32::from(glyph.y)),
                size: (w, h),
                solid,
                solids,
                coverage,
                edges,
                clear,
                ink,
            });
        }
    }
    // Glyphs with the same solid pixels, as a small K whose arms are all
    // edge is an I, are told apart by their edges: the most covered first.
    for list in map.values_mut() {
        list.sort_by_key(|t| {
            let covered: u32 = t.coverage.iter().map(|&c| u32::from(c)).sum();
            std::cmp::Reverse((t.ink, covered))
        });
    }
    let mut by_down: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for &(right, down) in map.keys() {
        by_down.entry(down).or_default().push((right, down));
    }
    for keys in by_down.values_mut() {
        keys.sort_unstable_by_key(|&(right, _)| std::cmp::Reverse(right));
    }
    Templates {
        exact: map,
        by_down,
    }
}

impl<'a> Frame<'a> {
    /// A frame read in the faces of its panel's scale step, or at scale 1 on
    /// a panel VANA cannot lay out.
    pub fn new(width: usize, height: usize, pixels: &'a [u16], look: Look) -> Self {
        let faces = vana::screens::scaled::Scaled::fitting(width, height)
            .map_or(&SCALE_1, |scaled| scaled.faces);
        Self {
            width,
            height,
            pixels,
            look,
            faces,
        }
    }

    fn face(&self, index: usize) -> &'static Face {
        self.faces.0[index]
    }

    /// Every text run on screen, top-to-bottom then left-to-right.
    pub fn read(&self) -> Vec<TextRun> {
        if self.pixels.len() < self.width * self.height {
            return Vec::new();
        }
        let glyphs = std::thread::scope(|threads| {
            let smooth = threads.spawn(|| self.smooth_glyphs());
            let mut glyphs = self.glyphs();
            glyphs.extend(smooth.join().expect("smooth glyph scan"));
            glyphs
        });
        let mut runs = self.group(glyphs);
        runs.retain(|run| !(run.text.len() == 1 && b"-.,:/_=".contains(&run.text.as_bytes()[0])));
        runs.retain(|run| !run.text.bytes().all(|b| b == b'_' || b == b' '));
        runs.sort_by_key(|run| (run.y, run.x));
        runs
    }

    /// Runs containing `needle` (exact substring of a run's text).
    pub fn find(&self, needle: &str) -> Vec<TextRun> {
        let mut runs = self.read();
        runs.retain(|run| run.text.contains(needle));
        runs
    }

    pub fn contains(&self, needle: &str) -> bool {
        !self.find(needle).is_empty()
    }

    /// All run texts joined with '\n' in reading order, for logs/reports.
    pub fn text(&self) -> String {
        let texts: Vec<String> = self.read().into_iter().map(|run| run.text).collect();
        texts.join("\n")
    }

    fn at(&self, x: usize, y: usize) -> u16 {
        self.pixels[y * self.width + x]
    }

    fn glyphs(&self) -> Vec<Glyph> {
        let lookup = Lookup::get();
        let scales: Vec<usize> = (1..=MAX_SCALE)
            .rev()
            .filter(|&scale| self.width >= 5 * scale && self.height >= 7 * scale)
            .collect();
        let pixels = &self.pixels[..self.width * self.height];
        let corners = Corners::new(self);
        let corners = &corners;
        let previous = LAST_SCAN
            .with(|last| last.borrow_mut().take())
            .filter(|scan| (scan.width, scan.height) == (self.width, self.height));
        let candidates: Vec<Vec<Glyph>> = match &previous {
            // A frame that changed in a few places is rescanned only there.
            Some(scan) => {
                let changed = changed_columns(&scan.pixels, pixels, self.width);
                scales
                    .iter()
                    .zip(&scan.candidates)
                    .map(|(&scale, old)| self.rescan(lookup, corners, scale, &changed, old))
                    .collect()
            }
            // Scanning every position at every scale is the slow part; the
            // scans are independent, so they run in parallel.
            None => std::thread::scope(|threads| {
                let scans: Vec<_> = scales
                    .iter()
                    .map(|&scale| threads.spawn(move || self.candidates(lookup, corners, scale)))
                    .collect();
                scans
                    .into_iter()
                    .map(|scan| scan.join().expect("glyph scan"))
                    .collect()
            }),
        };
        LAST_SCAN.with(|last| {
            *last.borrow_mut() = Some(Scan {
                width: self.width,
                height: self.height,
                pixels: previous.map_or_else(
                    || pixels.to_vec(),
                    |mut scan| {
                        scan.pixels.copy_from_slice(pixels);
                        scan.pixels
                    },
                ),
                candidates: candidates.clone(),
            });
        });
        // Claiming stays in order, largest scale first, so the result is the
        // same however the candidates were found.
        let mut claimed = vec![false; self.width * self.height];
        let mut found = Vec::new();
        for (scale, mut candidates) in scales.into_iter().zip(candidates) {
            // Richer glyphs first so a partial match cannot shadow ':' or similar.
            candidates.sort_by_key(|g| (std::cmp::Reverse(g.ink), g.y, g.x));
            for glyph in candidates {
                if self.overlaps(&claimed, glyph.x, glyph.y, scale) {
                    continue;
                }
                for row in glyph.y..glyph.y + 7 * scale {
                    let start = row * self.width + glyph.x;
                    claimed[start..start + 5 * scale].fill(true);
                }
                found.push(glyph);
            }
        }
        found
    }

    /// Every position where a 5x7 glyph of `scale` matches, claimed or not.
    fn candidates(&self, lookup: &Lookup, corners: &Corners, scale: usize) -> Vec<Glyph> {
        let dots = Dots::new(self.width, scale);
        let mut starts = Starts::new(corners);
        let mut candidates = Vec::new();
        for y in 0..=self.height - 7 * scale {
            for &x in starts.row(lookup, scale, y, 0, self.width - 5 * scale) {
                candidates.extend(self.glyph_at(lookup, &dots, x, y));
            }
        }
        candidates
    }

    /// `old` (this scale's candidates in the previous frame) with every
    /// position whose result could differ scanned again. `glyph_at` and
    /// `verify` read a cell, one dot beside it and the rows just above and
    /// below: columns `x - s .. x + 6s` and rows `y - 1 ..= y + 7s`.
    fn rescan(
        &self,
        lookup: &Lookup,
        corners: &Corners,
        s: usize,
        changed: &[Option<(usize, usize)>],
        old: &[Glyph],
    ) -> Vec<Glyph> {
        let last_x = self.width - 5 * s;
        let stale: Vec<Option<(usize, usize)>> = (0..=self.height - 7 * s)
            .map(|y| {
                let rows = y.saturating_sub(1)..(y + 7 * s + 1).min(self.height);
                let (lo, hi) = changed[rows].iter().flatten().fold(
                    None,
                    |span: Option<(usize, usize)>, &(a, b)| {
                        Some(span.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b))))
                    },
                )?;
                // The footprint meets lo..=hi when x - s <= hi and x + 6s > lo.
                let (from, to) = ((lo + 1).saturating_sub(6 * s), (hi + s).min(last_x));
                (from <= to).then_some((from, to))
            })
            .collect();
        let is_stale =
            |x: usize, y: usize| stale[y].is_some_and(|(from, to)| (from..=to).contains(&x));
        let mut candidates: Vec<Glyph> = old
            .iter()
            .copied()
            .filter(|g| !is_stale(g.x, g.y))
            .collect();
        let dots = Dots::new(self.width, s);
        let mut starts = Starts::new(corners);
        for (y, span) in stale.iter().enumerate() {
            if let Some((from, to)) = *span {
                for &x in starts.row(lookup, s, y, from, to) {
                    candidates.extend(self.glyph_at(lookup, &dots, x, y));
                }
            }
        }
        candidates
    }

    /// Anti-aliased glyphs: each starts at a solid pixel with no solid pixel of
    /// its colour to the left or above, and matches a template exactly.
    fn smooth_glyphs(&self) -> Vec<Glyph> {
        let templates = templates(self.faces);
        let (width, height) = (self.width, self.height);
        let blank = self.look.ink(0).0;
        let mut claimed = vec![false; width * height];
        let mut found = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let color = self.at(x, y);
                // Ink touching from the left or above starts elsewhere, unless
                // it belongs to a glyph already read, as touching capitals do.
                let joined =
                    |nx: usize, ny: usize| self.at(nx, ny) == color && !claimed[ny * width + nx];
                if color == blank
                    || claimed[y * width + x]
                    || (x > 0 && joined(x - 1, y))
                    || (y > 0 && joined(x, y - 1))
                {
                    continue;
                }
                let right = (x..width).take_while(|&cx| self.at(cx, y) == color).count();
                let down = (y..height)
                    .take_while(|&cy| self.at(x, cy) == color)
                    .count();
                // A run that continues into the next glyph, as in "TT", is
                // longer than the template's own.
                let exact = templates.exact.get(&(right, down)).into_iter().flatten();
                let joined = templates
                    .by_down
                    .get(&down)
                    .into_iter()
                    .flatten()
                    .filter(|&&(r, _)| r < right)
                    .flat_map(|key| &templates.exact[key]);
                for template in exact.chain(joined) {
                    let Some((left, top)) = self.matches(template, x, y, color, &claimed) else {
                        continue;
                    };
                    let (w, h) = template.size;
                    for row in top..top + h {
                        for col in left..left + w {
                            if template.solid[(row - top) * w + col - left] {
                                claimed[row * width + col] = true;
                            }
                        }
                    }
                    let glyph = self.face(template.face).glyph(template.character);
                    found.push(Glyph {
                        character: template.character,
                        x: (left as i32 - template.offset.0) as usize,
                        y: (top as i32 - template.offset.1) as usize,
                        scale: FACE_SCALE + template.face,
                        color,
                        ink: template.ink,
                        advance: usize::from(glyph.advance),
                    });
                    break;
                }
            }
        }
        found
    }

    /// The glyph box's top-left corner when `template` matches at an anchor.
    fn matches(
        &self,
        template: &Template,
        x: usize,
        y: usize,
        color: u16,
        claimed: &[bool],
    ) -> Option<(usize, usize)> {
        let left = x.checked_sub(template.anchor.0)?;
        let top = y.checked_sub(template.anchor.1)?;
        let (w, h) = template.size;
        if left + w > self.width || top + h > self.height {
            return None;
        }
        // Most anchors are lone edge pixels that no template's other solid
        // pixels fit; the full comparison below would read most of the box.
        if template
            .solids
            .iter()
            .any(|&i| self.at(left + i % w, top + i / w) != color)
        {
            return None;
        }
        for row in 0..h {
            for col in 0..w {
                let (px, py) = (left + col, top + row);
                let solid = self.at(px, py) == color;
                // Neighbours reach into the box: the glyph before is already
                // claimed, and the one after may touch the last two columns,
                // as the crossbars of "TT" do.
                let neighbour = solid && (claimed[py * self.width + px] || col + 2 >= w);
                if solid != template.solid[row * w + col] && !neighbour {
                    return None;
                }
            }
        }
        // Pen positions are whole pixels; a glyph left of the canvas is not text.
        if (left as i32) < template.offset.0 || (top as i32) < template.offset.1 {
            return None;
        }
        // The edges must blend from the background, or this is a patch of
        // some other glyph's edge colour shaped like a small glyph.
        let pixel = |i: usize| self.at(left + i % w, top + i / w);
        let edges = &template.edges;
        if edges.is_empty() {
            return Some((left, top));
        }
        let outside = if left > 0 {
            self.at(left - 1, top)
        } else {
            self.at(left + w, top)
        };
        // The commonest colour outside the glyph, since neighbours may reach in.
        // A box holds a handful of colours, so a short list beats a map.
        let mut counts: Vec<(u16, usize)> = Vec::with_capacity(8);
        for &i in &template.clear {
            let color = pixel(i);
            match counts.iter_mut().find(|(c, _)| *c == color) {
                Some((_, n)) => *n += 1,
                None => counts.push((color, 1)),
            }
        }
        let background = counts
            .into_iter()
            .max_by_key(|&(_, n)| n)
            .map_or(outside, |(c, _)| c);
        let blended = edges
            .iter()
            .filter(|&&i| {
                let mixed = blend(Shown(color), Shown(background), template.coverage[i]);
                pixel(i) == mixed.0
            })
            .count();
        (blended * 4 >= edges.len() * 3).then_some((left, top))
    }

    fn overlaps(&self, claimed: &[bool], x: usize, y: usize, scale: usize) -> bool {
        (y..y + 7 * scale).any(|row| {
            let start = row * self.width + x;
            claimed[start..start + 5 * scale].iter().any(|&c| c)
        })
    }

    fn glyph_at(&self, lookup: &Lookup, dots: &Dots, x: usize, y: usize) -> Option<Glyph> {
        let s = dots.scale;
        let base = y * self.width + x;
        let sample = |index: usize| self.pixels[base + dots.dots[index]];
        let first = sample(0);
        if s > 1
            && dots
                .corners
                .iter()
                .any(|&offset| self.pixels[base + offset] != first)
        {
            return None;
        }
        let mut keyed = [first; KEYS.len()];
        for i in 1..PROBES {
            keyed[i] = sample(KEYS[i]);
        }
        if keyed[..PROBES].iter().all(|&color| color == first) {
            return None;
        }
        for i in PROBES..KEYS.len() {
            keyed[i] = sample(KEYS[i]);
        }
        let mut inks = [(0_u64, 0_u16, 0_u8); PROBES];
        let mut count = 0;
        for (i, &color) in keyed[..PROBES].iter().enumerate() {
            if keyed[..i].contains(&color) {
                continue;
            }
            let key = keyed
                .iter()
                .enumerate()
                .fold(0, |key, (i, &c)| key | usize::from(c == color) << i);
            if !lookup.keyed(key) {
                continue;
            }
            let bits = (0..35).fold(0_u64, |bits, i| bits | u64::from(sample(i) == color) << i);
            if let Some(character) = lookup.character(bits) {
                inks[count] = (bits, color, character);
                count += 1;
            }
        }
        if count == 0 {
            return None;
        }
        // Ink plus at most three distinct off colours; busier cells are not text.
        let mut colors = [first; 4];
        let mut distinct = 1;
        for index in 1..35 {
            let color = sample(index);
            if !colors[..distinct].contains(&color) {
                if distinct == colors.len() {
                    return None;
                }
                colors[distinct] = color;
                distinct += 1;
            }
        }
        let inks = &mut inks[..count];
        inks.sort_unstable_by_key(|&(bits, ..)| bits.trailing_zeros());
        for &(bits, color, character) in inks.iter() {
            if self.verify(x, y, s, color, bits) {
                return Some(Glyph {
                    character,
                    x,
                    y,
                    scale: s,
                    color,
                    ink: bits.count_ones(),
                    advance: 6 * s,
                });
            }
        }
        None
    }

    fn verify(&self, x: usize, y: usize, s: usize, color: u16, bits: u64) -> bool {
        for index in 0..35 {
            let lit = bits >> index & 1 == 1;
            let (bx, by) = (x + (index % 5) * s, y + (index / 5) * s);
            let first = self.at(bx, by);
            let uniform =
                (by..by + s).all(|row| (bx..bx + s).all(|col| self.at(col, row) == first));
            if !uniform || (first == color) != lit {
                return false;
            }
        }
        let left = if x >= s { x - s } else { x };
        let right = (x + 6 * s).min(self.width);
        let clear_columns = |from: usize, to: usize| {
            (y..y + 7 * s).all(|row| (from..to).all(|col| self.at(col, row) != color))
        };
        let clear_rows = |row: usize| (x..x + 5 * s).all(|col| self.at(col, row) != color);
        clear_columns(left, x)
            && clear_columns(x + 5 * s, right)
            && (y == 0 || clear_rows(y - 1))
            && (y + 7 * s >= self.height || clear_rows(y + 7 * s))
    }

    fn cell_blank(&self, x: usize, y: usize, s: usize, color: u16) -> bool {
        let right = (x + 6 * s).min(self.width);
        (y..y + 7 * s).all(|row| (x..right).all(|col| self.at(col, row) != color))
    }

    fn group(&self, mut glyphs: Vec<Glyph>) -> Vec<TextRun> {
        glyphs.sort_by_key(|g| (g.y, g.scale, g.color, g.x));
        let mut runs: Vec<TextRun> = Vec::new();
        let mut last: Option<Glyph> = None;
        for glyph in glyphs {
            let character = glyph.character as char;
            if let (Some(previous), Some(run)) = (last, runs.last_mut()) {
                let same = (previous.y, previous.scale, previous.color)
                    == (glyph.y, glyph.scale, glyph.color);
                let step = glyph.x.wrapping_sub(previous.x);
                let s = glyph.scale;
                if same && s >= FACE_SCALE {
                    let space = usize::from(self.face(s - FACE_SCALE).glyph(b' ').advance);
                    let gap = step.wrapping_sub(previous.advance);
                    if gap == 0 || gap == space || gap == 2 * space {
                        for _ in 0..gap / space.max(1) {
                            run.text.push(' ');
                        }
                        run.text.push(character);
                        last = Some(glyph);
                        continue;
                    }
                }
                if same && s < FACE_SCALE && step == 6 * s {
                    run.text.push(character);
                    last = Some(glyph);
                    continue;
                }
                if same
                    && s < FACE_SCALE
                    && step == 12 * s
                    && self.cell_blank(previous.x + 6 * s, glyph.y, s, glyph.color)
                {
                    run.text.push(' ');
                    run.text.push(character);
                    last = Some(glyph);
                    continue;
                }
            }
            runs.push(TextRun {
                text: character.to_string(),
                x: glyph.x,
                y: glyph.y,
                // A face reads as the 5x7 scale of similar capital height.
                scale: if glyph.scale >= FACE_SCALE {
                    (usize::from(self.face(glyph.scale - FACE_SCALE).cap) / 7).max(1)
                } else {
                    glyph.scale
                },
                color: glyph.color,
                smooth: glyph.scale >= FACE_SCALE,
            });
            last = Some(glyph);
        }
        runs
    }
}

/// Pixel offsets from a 5x7 cell's top-left pixel at one scale: each dot's
/// top-left pixel, and the other three corners of the first dot. Most scanned
/// positions are rejected on a few of these, so they are computed once a scan.
struct Dots {
    scale: usize,
    dots: [usize; 35],
    corners: [usize; 3],
}

impl Dots {
    fn new(width: usize, scale: usize) -> Self {
        let edge = scale - 1;
        Self {
            scale,
            dots: std::array::from_fn(|index| (index / 5) * scale * width + (index % 5) * scale),
            corners: [edge, edge * width, edge * width + edge],
        }
    }
}

struct Corners {
    words: usize,
    bits: Vec<u64>,
}

impl Corners {
    fn new(frame: &Frame) -> Self {
        let (width, height) = (frame.width, frame.height);
        let words = width.div_ceil(64);
        let mut bits = vec![0_u64; words * height];
        for y in 0..height {
            let row = &frame.pixels[y * width..(y + 1) * width];
            let above = y
                .checked_sub(1)
                .map(|y| &frame.pixels[y * width..(y + 1) * width]);
            for x in 0..width {
                let color = row[x];
                let left = x == 0 || row[x - 1] != color;
                let up = above.is_none_or(|above| above[x] != color);
                if left && up {
                    bits[y * words + x / 64] |= 1 << (x % 64);
                }
            }
        }
        Self { words, bits }
    }
}

struct Starts<'a> {
    corners: &'a Corners,
    mask: Vec<u64>,
    columns: Vec<usize>,
}

impl<'a> Starts<'a> {
    fn new(corners: &'a Corners) -> Self {
        Self {
            corners,
            mask: vec![0; corners.words],
            columns: Vec::new(),
        }
    }

    fn row(&mut self, lookup: &Lookup, s: usize, y: usize, from: usize, to: usize) -> &[usize] {
        let words = self.corners.words;
        self.mask.fill(0);
        for &(row, column) in &lookup.first_dots {
            let start = (y + row * s) * words;
            let source = &self.corners.bits[start..start + words];
            let (skip, shift) = (column * s / 64, column * s % 64);
            for (i, word) in self.mask.iter_mut().enumerate() {
                let low = source.get(i + skip).map_or(0, |&w| w >> shift);
                let high = match shift {
                    0 => 0,
                    _ => source.get(i + skip + 1).map_or(0, |&w| w << (64 - shift)),
                };
                *word |= low | high;
            }
        }
        for x in 0..s {
            self.mask[x / 64] |= 1 << (x % 64);
        }
        self.columns.clear();
        for i in from / 64..=to / 64 {
            let mut word = self.mask[i];
            while word != 0 {
                let x = i * 64 + word.trailing_zeros() as usize;
                word &= word - 1;
                if (from..=to).contains(&x) {
                    self.columns.push(x);
                }
            }
        }
        &self.columns
    }
}

/// The last frame this thread read and its 5x7 candidates per scale.
/// Scenarios read the same screen many times as only a clock or a reading
/// changes, so most of a frame's candidates carry over.
struct Scan {
    width: usize,
    height: usize,
    pixels: Vec<u16>,
    candidates: Vec<Vec<Glyph>>,
}

thread_local! {
    static LAST_SCAN: RefCell<Option<Scan>> = const { RefCell::new(None) };
}

/// Per row, the first and last column that differ between two frames.
fn changed_columns(before: &[u16], after: &[u16], width: usize) -> Vec<Option<(usize, usize)>> {
    before
        .chunks_exact(width)
        .zip(after.chunks_exact(width))
        .map(|(old, new)| {
            let first = old.iter().zip(new).position(|(a, b)| a != b)?;
            let last = old.iter().zip(new).rposition(|(a, b)| a != b)?;
            Some((first, last))
        })
        .collect()
}

fn mask(rows: &[u8; 7]) -> u64 {
    let mut bits = 0;
    for (row, &value) in rows.iter().enumerate() {
        for column in 0..5 {
            if value & (1 << (4 - column)) != 0 {
                bits |= 1 << (row * 5 + column);
            }
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware_shell::rendering::text::{number, text};

    const W: usize = 240;
    const H: usize = 320;
    const BG: u16 = 0x2104;
    const WHITE: u16 = 0xffff;
    const CYAN: u16 = 0x07ff;
    const GREEN: u16 = 0x07e0;
    const RED: u16 = 0xf800;

    fn canvas(bg: u16) -> Vec<u16> {
        vec![bg; W * H]
    }

    fn draw(pixels: &mut [u16], x: usize, y: usize, scale: usize, color: u16, label: &str) {
        for py in y..y + 7 * scale {
            for px in x..x + 6 * scale * label.len() {
                if text(px, py, x, y, scale, label.as_bytes()) {
                    pixels[py * W + px] = color;
                }
            }
        }
    }

    fn fill(pixels: &mut [u16], x: usize, y: usize, w: usize, h: usize, color: u16) {
        for py in y..y + h {
            pixels[py * W + x..py * W + x + w].fill(color);
        }
    }

    fn frame(pixels: &[u16]) -> Frame<'_> {
        Frame::new(W, H, pixels, Look::PLAIN)
    }

    fn run(text: &str, x: usize, y: usize, scale: usize, color: u16) -> TextRun {
        TextRun {
            text: text.into(),
            x,
            y,
            scale,
            color,
            smooth: false,
        }
    }

    fn smooth(
        pixels: &mut [u16],
        x: usize,
        y: usize,
        face: &'static Face,
        color: u16,
        label: &str,
    ) {
        smooth_on(pixels, W, x, y, face, color, label);
    }

    fn smooth_on(
        pixels: &mut [u16],
        width: usize,
        x: usize,
        y: usize,
        face: &'static Face,
        color: u16,
        label: &str,
    ) {
        use firmware_shell::rendering::font::{Align, Label, Line};
        let label = Label::new(
            Line::<24>::new(face, label.as_bytes()),
            x,
            y,
            Align::Left,
            color,
        );
        for py in y..label.bottom() {
            for px in x..label.right() + 2 {
                let bg = pixels[py * width + px];
                if let Some(c) = label.pixel(Look::PLAIN, px, py, Shown(bg)) {
                    pixels[py * width + px] = c.0;
                }
            }
        }
    }

    /// A read with no previous frame on this thread scans everything.
    fn fresh(pixels: &[u16]) -> Vec<TextRun> {
        LAST_SCAN.with(|last| last.borrow_mut().take());
        let runs = frame(pixels).read();
        LAST_SCAN.with(|last| last.borrow_mut().take());
        runs
    }

    #[test]
    fn rereading_a_changed_frame_matches_a_fresh_read() {
        use firmware_shell::rendering::font::FACES;
        let mut pixels = canvas(BG);
        fill(&mut pixels, 0, 0, W, 24, CYAN);
        draw(&mut pixels, 4, 6, 2, BG, "7:18 REC");
        draw(&mut pixels, 10, 60, 6, WHITE, "28.0");
        smooth(&mut pixels, 10, 140, FACES[0], GREEN, "220 W");
        draw(&mut pixels, 200, 300, 1, RED, "Z4");
        LAST_SCAN.with(|last| last.borrow_mut().take());
        let steps: [fn(&mut Vec<u16>); 9] = [
            // The clock ticks: a few changed pixels inside a bar.
            |p| draw(p, 4, 6, 2, BG, "7:19 REC"),
            // A large value changes, then grows into the next cell.
            |p| {
                fill(p, 10, 60, 160, 42, BG);
                draw(p, 10, 60, 6, WHITE, "31.5")
            },
            |p| draw(p, 10, 60, 6, WHITE, "131.5"),
            // A glyph is drawn over, then a neighbour appears beside one.
            |p| fill(p, 22, 66, 9, 9, WHITE),
            |p| draw(p, 212, 300, 1, RED, "X"),
            // Text at the frame's edges and a smooth label changing.
            |p| draw(p, 0, H - 7, 1, WHITE, "EDGE"),
            |p| {
                fill(p, 10, 140, 120, 40, BG);
                smooth(p, 10, 140, FACES[0], GREEN, "235 W")
            },
            |p| fill(p, 0, 0, W, H, BG),
            |p| draw(p, 30, 30, 3, CYAN, "BACK"),
        ];
        let first = frame(&pixels).read();
        assert_eq!(first, fresh(&pixels));
        frame(&pixels).read();
        for (i, step) in steps.iter().enumerate() {
            step(&mut pixels);
            let incremental = frame(&pixels).read();
            let expected = fresh(&pixels);
            assert_eq!(incremental, expected, "step {i}");
            // Leave this frame as the previous one for the next step.
            frame(&pixels).read();
        }
    }

    #[test]
    fn reads_anti_aliased_faces_beside_bitmap_text() {
        use firmware_shell::rendering::font::{BODY, CAPTION, HERO, LARGE, TITLE, VALUE};
        let mut p = canvas(0);
        smooth(&mut p, 10, 4, &CAPTION, 0x8410, "REC 9:41 AM/PM 100%");
        smooth(&mut p, 6, 24, &TITLE, WHITE, "RIDE");
        smooth(&mut p, 10, 70, &BODY, CYAN, "SCAN DONE - PICK");
        smooth(&mut p, 10, 100, &HERO, CYAN, "29.9");
        smooth(&mut p, 10, 170, &LARGE, 0xffe0, "1250");
        smooth(&mut p, 130, 170, &VALUE, RED, "-3.5%");
        smooth(&mut p, 10, 230, &VALUE, WHITE, "01:02:05");
        smooth(&mut p, 10, 270, &BODY, WHITE, "5.00 KM  00:10:00");
        draw(&mut p, 150, 230, 2, GREEN, "OK");
        let texts: Vec<_> = frame(&p).read().into_iter().map(|r| r.text).collect();
        assert_eq!(
            texts,
            [
                "REC 9:41 AM/PM 100%",
                "RIDE",
                "SCAN DONE - PICK",
                "29.9",
                "1250",
                "-3.5%",
                "01:02:05",
                "OK",
                "5.00 KM  00:10:00",
            ]
        );
    }

    #[test]
    fn reads_a_small_panel_in_its_scale_steps_faces() {
        use firmware_shell::rendering::font::{FaceRole, SCALE_0_75};
        let (width, height) = (180, 240);
        let mut p = vec![0; width * height];
        let face = |role| SCALE_0_75.get(role);
        smooth_on(
            &mut p,
            width,
            8,
            4,
            face(FaceRole::Caption),
            WHITE,
            "REC 100%",
        );
        smooth_on(&mut p, width, 8, 40, face(FaceRole::Hero), CYAN, "29.9");
        smooth_on(&mut p, width, 8, 110, face(FaceRole::Body), WHITE, "KM/H");
        smooth_on(
            &mut p,
            width,
            8,
            150,
            face(FaceRole::Value),
            GREEN,
            "01:02:05",
        );
        smooth_on(
            &mut p,
            width,
            8,
            190,
            face(FaceRole::Caption),
            WHITE,
            "BACK",
        );
        let frame = Frame::new(width, height, &p, Look::PLAIN);
        assert!(core::ptr::eq(frame.faces, &SCALE_0_75));
        let texts: Vec<_> = frame.read().into_iter().map(|r| r.text).collect();
        assert_eq!(texts, ["REC 100%", "29.9", "KM/H", "01:02:05", "BACK"]);
        assert!(frame.contains("KM/H"));
    }

    #[test]
    fn reads_each_sign_and_totals_face_as_itself() {
        use firmware_shell::rendering::font::{
            BODY, FOOT_20, SIGN_13, SIGN_14, SIGN_18, SIGN_20, SIGN_27, SIGN_32, SIGN_50, TITLE,
            TOTAL_11, TOTAL_30, TOTAL_60,
        };
        const TURN: u16 = 0x0b7c;
        const OFF_COURSE: u16 = 0xd3a0;
        const ARRIVED: u16 = 0x1cc8;
        let cases: [(&Face, u16, &str); 23] = [
            (&SIGN_50, TURN, "120"),
            (&SIGN_50, OFF_COURSE, "1.4"),
            (&SIGN_32, TURN, "250"),
            (&SIGN_32, TURN, "40"),
            (&SIGN_27, ARRIVED, "ARRIVED"),
            (&SIGN_20, TURN, "M"),
            (&SIGN_20, TURN, "KM"),
            (&SIGN_20, BG, "6.4"),
            (&SIGN_20, ARRIVED, "ARRIVED"),
            (&SIGN_27, OFF_COURSE, "TURN"),
            (&SIGN_27, OFF_COURSE, "AROUND"),
            (&SIGN_20, OFF_COURSE, "AROUND"),
            (&SIGN_18, OFF_COURSE, "TURN BACK"),
            (&SIGN_14, OFF_COURSE, "TURN BACK"),
            (&SIGN_14, TURN, "KM"),
            (&SIGN_13, BG, "KM"),
            (&TOTAL_60, 0, "56.20"),
            (&TOTAL_30, 0, "27.1"),
            (&TOTAL_30, 0, "168"),
            (&TOTAL_11, 0, "AVG KM/H"),
            (&TOTAL_11, 0, "RIDE TIME"),
            (&FOOT_20, 0, "12.4"),
            (&FOOT_20, 0, "0.8"),
        ];
        let index = |face: &Face| {
            SCALE_1
                .0
                .iter()
                .position(|&f| core::ptr::eq(f, face))
                .unwrap()
        };
        for (face, fill, text) in cases {
            let mut p = canvas(fill);
            smooth(&mut p, 12, 30, face, WHITE, text);
            let texts: Vec<_> = fresh(&p).into_iter().map(|r| r.text).collect();
            assert_eq!(texts, [text], "{text} in face {}", index(face));
            let glyphs = frame(&p).smooth_glyphs();
            let spelled = glyphs
                .iter()
                .filter(|g| text.as_bytes().contains(&g.character));
            assert_eq!(spelled.clone().count(), text.replace(' ', "").len());
            assert!(
                spelled.clone().all(|g| g.scale == FACE_SCALE + index(face)),
                "{text} in face {} reads as its own face",
                index(face)
            );
        }
        for (face, other, text) in [(&SIGN_32, &TITLE, "250"), (&SIGN_20, &BODY, "6.4 KM")] {
            let mut p = canvas(0);
            smooth(&mut p, 12, 30, face, WHITE, text);
            smooth(&mut p, 12, 120, other, WHITE, text);
            let faces: Vec<_> = frame(&p)
                .smooth_glyphs()
                .iter()
                .filter(|g| text.as_bytes().contains(&g.character))
                .map(|g| (g.y < 100, g.scale - FACE_SCALE))
                .collect();
            assert!(faces.contains(&(true, index(face))));
            assert!(faces.contains(&(false, index(other))));
            assert!(
                faces
                    .iter()
                    .all(|&(top, f)| f == if top { index(face) } else { index(other) })
            );
        }
    }

    #[test]
    fn table_matches_shared_font() {
        for byte in 0..=255_u8 {
            let rows = GLYPHS
                .iter()
                .find(|&&(c, _)| c == byte)
                .map_or([0; 7], |&(_, rows)| rows);
            for scale in [1, 2] {
                for y in 0..7 * scale {
                    for x in 0..6 * scale {
                        let expected =
                            x < 5 * scale && rows[y / scale] & (1 << (4 - x / scale)) != 0;
                        assert_eq!(
                            text(x, y, 0, 0, scale, &[byte]),
                            expected,
                            "byte {byte} at {x},{y} scale {scale}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn probes_separate_every_glyph_from_blank() {
        for &(character, rows) in &GLYPHS {
            let bits = mask(&rows);
            let lit = KEYS[..PROBES]
                .iter()
                .filter(|&&index| bits >> index & 1 == 1)
                .count();
            assert!(0 < lit && lit < PROBES, "{}", character as char);
        }
    }

    #[test]
    fn corner_starts_keep_every_position_a_glyph_matches() {
        let mut p = canvas(BG);
        let mut seed = 7_u32;
        for py in 200..H {
            for px in 0..W {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                p[py * W + px] = [BG, WHITE, CYAN][(seed >> 16) as usize % 3];
            }
        }
        draw(&mut p, 0, 0, 1, WHITE, "AJ1:4/-,.+Q");
        fill(&mut p, 0, 12, 1, 14, WHITE);
        draw(&mut p, 1, 12, 2, WHITE, "-T,");
        draw(&mut p, 0, 30, 3, CYAN, "B.7");
        draw(&mut p, 60, 30, 3, WHITE, "OK");
        draw(&mut p, 0, 60, 4, GREEN, "+4");
        draw(&mut p, 3, 100, 8, RED, "J");
        for (i, scale) in [1, 2, 3].into_iter().enumerate() {
            draw(&mut p, 10 + i * 70, 210 + i * 30, scale, RED, "M:/.");
        }
        let frame = frame(&p);
        let lookup = Lookup::get();
        let corners = Corners::new(&frame);
        for scale in 1..=MAX_SCALE {
            let dots = Dots::new(W, scale);
            let every: Vec<_> = (0..=H - 7 * scale)
                .flat_map(|y| (0..=W - 5 * scale).map(move |x| (x, y)))
                .filter(|&(x, y)| frame.glyph_at(lookup, &dots, x, y).is_some())
                .collect();
            let started: Vec<_> = frame
                .candidates(lookup, &corners, scale)
                .iter()
                .map(|glyph| (glyph.x, glyph.y))
                .collect();
            assert_eq!(started, every, "scale {scale}");
            if [1, 2, 3, 4, 8].contains(&scale) {
                assert!(!every.is_empty(), "scale {scale}");
            }
        }
    }

    #[test]
    fn reads_mixed_scales_and_colors() {
        let mut p = canvas(BG);
        draw(&mut p, 10, 40, 3, WHITE, "CONNECTED");
        draw(&mut p, 12, 100, 2, RED, "DATA LOST");
        draw(&mut p, 8, 150, 1, GREEN, "12:05 PM");
        draw(&mut p, 20, 200, 6, WHITE, "142");
        draw(&mut p, 140, 200, 2, CYAN, "BPM");
        draw(&mut p, 5, 260, 2, WHITE, "GRADE +4.5%");
        let runs = frame(&p).read();
        assert_eq!(
            runs,
            vec![
                run("CONNECTED", 10, 40, 3, WHITE),
                run("DATA LOST", 12, 100, 2, RED),
                run("12:05 PM", 8, 150, 1, GREEN),
                run("142", 20, 200, 6, WHITE),
                run("BPM", 140, 200, 2, CYAN),
                run("GRADE +4.5%", 5, 260, 2, WHITE),
            ]
        );
        assert_eq!(
            frame(&p).text(),
            "CONNECTED\nDATA LOST\n12:05 PM\n142\nBPM\nGRADE +4.5%"
        );
    }

    #[test]
    fn reads_number_fields() {
        let mut p = canvas(BG);
        for py in 20..34 {
            for px in 10..70 {
                if number(px, py, 10, 20, 2, 142, 5) {
                    p[py * W + px] = WHITE;
                }
            }
        }
        assert_eq!(frame(&p).read(), vec![run("142", 34, 20, 2, WHITE)]);
    }

    #[test]
    fn reads_text_on_tile() {
        let mut p = canvas(BG);
        fill(&mut p, 20, 60, 100, 50, CYAN);
        draw(&mut p, 26, 70, 2, 0x0000, "142 BPM");
        assert_eq!(frame(&p).read(), vec![run("142 BPM", 26, 70, 2, 0)]);
    }

    #[test]
    fn ignores_lines_and_boxes() {
        let mut p = canvas(BG);
        fill(&mut p, 0, 29, W, 1, WHITE);
        fill(&mut p, 20, 50, 100, 2, WHITE);
        fill(&mut p, 20, 50, 2, 60, WHITE);
        fill(&mut p, 118, 50, 2, 60, WHITE);
        fill(&mut p, 20, 108, 100, 2, WHITE);
        fill(&mut p, 150, 60, 1, 40, WHITE);
        draw(&mut p, 40, 70, 2, WHITE, "OK");
        assert_eq!(frame(&p).read(), vec![run("OK", 40, 70, 2, WHITE)]);
    }

    #[test]
    fn splits_distant_strings_on_one_baseline() {
        let mut p = canvas(BG);
        draw(&mut p, 10, 80, 2, WHITE, "LEFT");
        draw(&mut p, 120, 80, 2, WHITE, "RIGHT");
        assert_eq!(
            frame(&p).read(),
            vec![
                run("LEFT", 10, 80, 2, WHITE),
                run("RIGHT", 120, 80, 2, WHITE)
            ]
        );
    }

    #[test]
    fn find_and_contains_match_substrings() {
        let mut p = canvas(BG);
        draw(&mut p, 10, 40, 3, WHITE, "CONNECTED");
        draw(&mut p, 10, 100, 1, RED, "NO DATA");
        let f = frame(&p);
        assert!(f.contains("NECT"));
        assert!(f.contains("NO DATA"));
        assert!(!f.contains("LOST"));
        assert_eq!(f.find("DATA"), vec![run("NO DATA", 10, 100, 1, RED)]);
        assert!(f.find("CONNECTED!").is_empty());
    }

    #[test]
    fn empty_frame_reads_nothing() {
        let p = canvas(BG);
        assert!(frame(&p).read().is_empty());
        assert_eq!(frame(&p).text(), "");
    }

    #[test]
    #[ignore = "timing measurement"]
    fn reads_full_frame_quickly() {
        let mut p = canvas(BG);
        for (i, scale) in [1, 2, 3, 2, 1, 2, 3, 1].into_iter().enumerate() {
            draw(&mut p, 4, 10 + i * 36, scale, WHITE, "GRADE 142 BPM");
        }
        let start = std::time::Instant::now();
        let runs = frame(&p).read();
        println!("read {} runs in {:?}", runs.len(), start.elapsed());
        assert_eq!(runs.len(), 8);
    }
}
