//! Recovers text from an RGB565 frame drawn with the shared 5x7 bitmap font.

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
    pub scale: usize,
    pub color: u16,
}

#[derive(Clone, Copy)]
struct Glyph {
    character: u8,
    x: usize,
    y: usize,
    scale: usize,
    color: u16,
    ink: u32,
}

impl Frame<'_> {
    /// Every text run on screen, top-to-bottom then left-to-right.
    pub fn read(&self) -> Vec<TextRun> {
        if self.pixels.len() < self.width * self.height {
            return Vec::new();
        }
        let mut runs = self.group(self.glyphs());
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
        let mut claimed = vec![false; self.width * self.height];
        let mut found = Vec::new();
        for scale in (1..=MAX_SCALE).rev() {
            if self.width < 5 * scale || self.height < 7 * scale {
                continue;
            }
            let mut candidates = Vec::new();
            for y in 0..=self.height - 7 * scale {
                for x in 0..=self.width - 5 * scale {
                    candidates.extend(self.glyph_at(&lookup, x, y, scale));
                }
            }
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
                if same && step == 6 * s {
                    run.text.push(character);
                    last = Some(glyph);
                    continue;
                }
                if same
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
                scale: glyph.scale,
                color: glyph.color,
            });
            last = Some(glyph);
        }
        runs
    }
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
