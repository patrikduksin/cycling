//! Recovers text from an RGB565 frame drawn with the shared 5x7 bitmap font
//! or the anti-aliased faces of `firmware_shell::rendering::font`.
use firmware_shell::rendering::font::{FACES, Face, blend};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

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

/// Dot indices (row * 5 + column) where every glyph has both lit and unlit dots,
/// so a cell uniform across them cannot hold a glyph.
const PROBES: [usize; 5] = [0, 7, 5, 28, 17];

pub struct Frame<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a [u16],
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
    /// Coverage 0..=15 of every box pixel.
    coverage: Vec<u8>,
    /// Box pixels the glyph only partly covers, and those it leaves clear.
    edges: Vec<usize>,
    clear: Vec<usize>,
    ink: u32,
}

/// Every face's templates, by the solid run right of and below the anchor.
struct Templates {
    exact: HashMap<(usize, usize), Vec<Template>>,
    /// Keys by `down`, longest `right` first: a run that continues into the
    /// next glyph, as in "TT", is longer than its template's own.
    by_down: HashMap<usize, Vec<(usize, usize)>>,
}

/// Templates keyed by the solid run right of and below the anchor.
fn templates() -> &'static Templates {
    static TEMPLATES: OnceLock<Templates> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let mut map: HashMap<(usize, usize), Vec<Template>> = HashMap::new();
        for (face_index, face) in FACES.iter().enumerate() {
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
                let ink = solid.iter().filter(|&&s| s).count() as u32;
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
                    coverage,
                    edges,
                    clear,
                    ink,
                });
            }
        }
        for list in map.values_mut() {
            list.sort_by_key(|t| std::cmp::Reverse(t.ink));
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
    })
}

fn face(index: usize) -> &'static Face {
    FACES[index]
}

impl Frame<'_> {
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
        runs.retain(|run| !(run.text.len() == 1 && b"-.,:/".contains(&run.text.as_bytes()[0])));
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
        let mut lookup: Vec<(u64, u8)> = GLYPHS
            .iter()
            .map(|&(character, rows)| (mask(&rows), character))
            .collect();
        lookup.sort_unstable();
        let scales: Vec<usize> = (1..=MAX_SCALE)
            .rev()
            .filter(|&scale| self.width >= 5 * scale && self.height >= 7 * scale)
            .collect();
        let pixels = &self.pixels[..self.width * self.height];
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
                    .map(|(&scale, old)| self.rescan(&lookup, scale, &changed, old))
                    .collect()
            }
            // Scanning every position at every scale is the slow part; the
            // scans are independent, so they run in parallel.
            None => std::thread::scope(|threads| {
                let scans: Vec<_> = scales
                    .iter()
                    .map(|&scale| {
                        let lookup = &lookup;
                        threads.spawn(move || self.candidates(lookup, scale))
                    })
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
    fn candidates(&self, lookup: &[(u64, u8)], scale: usize) -> Vec<Glyph> {
        let mut candidates = Vec::new();
        for y in 0..=self.height - 7 * scale {
            for x in 0..=self.width - 5 * scale {
                candidates.extend(self.glyph_at(lookup, x, y, scale));
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
        lookup: &[(u64, u8)],
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
        for (y, span) in stale.iter().enumerate() {
            if let Some((from, to)) = *span {
                for x in from..=to {
                    candidates.extend(self.glyph_at(lookup, x, y, s));
                }
            }
        }
        candidates
    }

    /// Anti-aliased glyphs: each starts at a solid pixel with no solid pixel of
    /// its colour to the left or above, and matches a template exactly.
    fn smooth_glyphs(&self) -> Vec<Glyph> {
        let templates = templates();
        let (width, height) = (self.width, self.height);
        let mut claimed = vec![false; width * height];
        let mut found = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let color = self.at(x, y);
                // Ink touching from the left or above starts elsewhere, unless
                // it belongs to a glyph already read, as touching capitals do.
                let joined =
                    |nx: usize, ny: usize| self.at(nx, ny) == color && !claimed[ny * width + nx];
                if color == 0
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
                    let glyph = face(template.face).glyph(template.character);
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
            .filter(|&&i| pixel(i) == blend(color, background, template.coverage[i]))
            .count();
        (blended * 4 >= edges.len() * 3).then_some((left, top))
    }

    fn overlaps(&self, claimed: &[bool], x: usize, y: usize, scale: usize) -> bool {
        (y..y + 7 * scale).any(|row| {
            let start = row * self.width + x;
            claimed[start..start + 5 * scale].iter().any(|&c| c)
        })
    }

    fn glyph_at(&self, lookup: &[(u64, u8)], x: usize, y: usize, s: usize) -> Option<Glyph> {
        let sample = |index: usize| self.at(x + (index % 5) * s, y + (index / 5) * s);
        let first = sample(0);
        if s > 1
            && [(s - 1, 0), (0, s - 1), (s - 1, s - 1)]
                .iter()
                .any(|&(dx, dy)| self.at(x + dx, y + dy) != first)
        {
            return None;
        }
        if PROBES.iter().all(|&index| sample(index) == first) {
            return None;
        }
        // Ink plus at most three distinct off colours; busier cells are not text.
        let mut colors = [first; 4];
        let mut masks = [0_u64; 4];
        let mut count = 1;
        for index in 0..35 {
            let color = sample(index);
            let slot = match colors[..count].iter().position(|&c| c == color) {
                Some(slot) => slot,
                None if count < 4 => {
                    colors[count] = color;
                    count += 1;
                    count - 1
                }
                None => return None,
            };
            masks[slot] |= 1 << index;
        }
        for (&color, &bits) in colors[..count].iter().zip(&masks) {
            let Ok(found) = lookup.binary_search_by_key(&bits, |&(mask, _)| mask) else {
                continue;
            };
            let character = lookup[found].1;
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
                    let space = usize::from(face(s - FACE_SCALE).glyph(b' ').advance);
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
                    (usize::from(face(glyph.scale - FACE_SCALE).cap) / 7).max(1)
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
        Frame {
            width: W,
            height: H,
            pixels,
        }
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
                let bg = pixels[py * W + px];
                if let Some(c) = label.pixel(px, py, bg) {
                    pixels[py * W + px] = c;
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
            let lit = PROBES
                .iter()
                .filter(|&&index| bits >> index & 1 == 1)
                .count();
            assert!(0 < lit && lit < PROBES.len(), "{}", character as char);
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
