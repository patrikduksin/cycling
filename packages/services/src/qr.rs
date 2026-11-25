//! QR code encoder for the pairing link: byte mode, version 4, error correction
//! level L (ISO/IEC 18004). The mask is chosen by the standard penalty rules.

/// Modules per side of a version 4 code, quiet zone excluded.
pub const SIZE: usize = 33;

const DATA_CODEWORDS: usize = 80;
const ECC_CODEWORDS: usize = 20;
const CODEWORDS: usize = DATA_CODEWORDS + ECC_CODEWORDS;
/// Bytes left after the 4-bit mode and 8-bit count.
const CAPACITY: usize = DATA_CODEWORDS - 2;
const ALIGNMENT: usize = 26;
const FORMAT_L: u32 = 0b01;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Code {
    rows: [u64; SIZE],
}

impl Code {
    /// Whether the module at column `x`, row `y` is dark.
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.rows[y] >> x & 1 == 1
    }
}

/// The code for `text`, or `None` when it is longer than 78 bytes.
pub fn encode(text: &[u8]) -> Option<Code> {
    if text.len() > CAPACITY {
        return None;
    }
    let mut matrix = Matrix::with_function_patterns();
    matrix.place(&codewords(text));
    (0..8).map(|mask| matrix.masked(mask)).min_by_key(penalty)
}

/// Data with its padding, then the error correction of its single block.
fn codewords(text: &[u8]) -> [u8; CODEWORDS] {
    let mut out = [0; CODEWORDS];
    // The 4-bit mode puts every later byte across a nibble boundary; the 4-bit
    // terminator fills the last low nibble.
    let mut previous = text.len() as u8;
    out[0] = 0x40 | previous >> 4;
    for (slot, &byte) in out[1..].iter_mut().zip(text) {
        *slot = previous << 4 | byte >> 4;
        previous = byte;
    }
    out[text.len() + 1] = previous << 4;
    for (i, slot) in out[text.len() + 2..DATA_CODEWORDS].iter_mut().enumerate() {
        *slot = if i % 2 == 0 { 0xEC } else { 0x11 };
    }
    let ecc = rs_remainder(&out[..DATA_CODEWORDS]);
    out[DATA_CODEWORDS..].copy_from_slice(&ecc);
    out
}

fn gf_multiply(x: u8, y: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= u16::from(y >> i & 1) * u16::from(x);
    }
    z as u8
}

fn rs_remainder(data: &[u8]) -> [u8; ECC_CODEWORDS] {
    let mut divisor = [0; ECC_CODEWORDS];
    divisor[ECC_CODEWORDS - 1] = 1;
    let mut root = 1;
    for _ in 0..ECC_CODEWORDS {
        for j in 0..ECC_CODEWORDS {
            divisor[j] = gf_multiply(divisor[j], root);
            if j + 1 < ECC_CODEWORDS {
                divisor[j] ^= divisor[j + 1];
            }
        }
        root = gf_multiply(root, 0x02);
    }

    let mut result = [0; ECC_CODEWORDS];
    for &byte in data {
        let factor = byte ^ result[0];
        result.copy_within(1.., 0);
        result[ECC_CODEWORDS - 1] = 0;
        for (r, &coefficient) in result.iter_mut().zip(&divisor) {
            *r ^= gf_multiply(coefficient, factor);
        }
    }
    result
}

#[derive(Clone, Copy)]
struct Matrix {
    dark: [u64; SIZE],
    function: [u64; SIZE],
}

impl Matrix {
    fn with_function_patterns() -> Self {
        let mut matrix = Self {
            dark: [0; SIZE],
            function: [0; SIZE],
        };
        for i in 0..SIZE {
            matrix.set(6, i, i % 2 == 0);
            matrix.set(i, 6, i % 2 == 0);
        }
        for (cx, cy) in [(3, 3), (SIZE - 4, 3), (3, SIZE - 4)] {
            for dy in -4..=4_isize {
                for dx in -4..=4_isize {
                    let (x, y) = (cx as isize + dx, cy as isize + dy);
                    if (0..SIZE as isize).contains(&x) && (0..SIZE as isize).contains(&y) {
                        let ring = dx.abs().max(dy.abs());
                        matrix.set(x as usize, y as usize, ring != 2 && ring != 4);
                    }
                }
            }
        }
        for dy in -2..=2_isize {
            for dx in -2..=2_isize {
                let (x, y) = (
                    (ALIGNMENT as isize + dx) as usize,
                    (ALIGNMENT as isize + dy) as usize,
                );
                matrix.set(x, y, dx.abs().max(dy.abs()) != 1);
            }
        }
        matrix.draw_format(0);
        matrix
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        let bit = 1 << x;
        self.function[y] |= bit;
        if dark {
            self.dark[y] |= bit;
        } else {
            self.dark[y] &= !bit;
        }
    }

    fn draw_format(&mut self, mask: u32) {
        let data = FORMAT_L << 3 | mask;
        let mut remainder = data;
        for _ in 0..10 {
            remainder = (remainder << 1) ^ ((remainder >> 9) * 0x537);
        }
        let bits = (data << 10 | remainder) ^ 0x5412;
        let bit = |i: usize| bits >> i & 1 == 1;

        for i in 0..6 {
            self.set(8, i, bit(i));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set(SIZE - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set(8, SIZE - 15 + i, bit(i));
        }
        self.set(8, SIZE - 8, true);
    }

    /// Codeword bits in the two-column zigzag from the bottom right; the
    /// remainder modules after the last codeword stay light.
    fn place(&mut self, data: &[u8; CODEWORDS]) {
        let mut at = 0;
        let mut right = SIZE - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            let upward = (right + 1) & 2 == 0;
            for vertical in 0..SIZE {
                let y = if upward {
                    SIZE - 1 - vertical
                } else {
                    vertical
                };
                for x in [right, right - 1] {
                    if self.function[y] >> x & 1 == 0 && at < CODEWORDS * 8 {
                        if data[at >> 3] >> (7 - (at & 7)) & 1 == 1 {
                            self.dark[y] |= 1 << x;
                        }
                        at += 1;
                    }
                }
            }
            right = right.saturating_sub(2);
        }
    }

    fn masked(mut self, mask: u32) -> Code {
        for y in 0..SIZE {
            let flip = (0..SIZE)
                .filter(|&x| flipped(mask, x, y))
                .fold(0u64, |flip, x| flip | 1 << x);
            self.dark[y] ^= flip & !self.function[y];
        }
        self.draw_format(mask);
        Code { rows: self.dark }
    }
}

fn flipped(mask: u32, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (x + y).is_multiple_of(3),
        4 => (x / 3 + y / 2).is_multiple_of(2),
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
        _ => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
    }
}

fn penalty(code: &Code) -> u32 {
    let rows = &code.rows;
    let mut score: u32 = rows.iter().map(|&row| line_penalty(row)).sum();
    for x in 0..SIZE {
        let column = (0..SIZE).fold(0u64, |column, y| column | (rows[y] >> x & 1) << y);
        score += line_penalty(column);
    }
    let inner = (1u64 << (SIZE - 1)) - 1;
    for pair in rows.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let same = !(a ^ b) & !(a ^ a >> 1) & !(b ^ b >> 1) & inner;
        score += 3 * same.count_ones();
    }
    let total = (SIZE * SIZE) as u32;
    let darks: u32 = rows.iter().map(|row| row.count_ones()).sum();
    let k = (darks * 20)
        .abs_diff(total * 10)
        .div_ceil(total)
        .saturating_sub(1);
    score + 10 * k
}

/// Rules 1 and 3 for one row or column; the border beyond it counts as light.
fn line_penalty(line: u64) -> u32 {
    let mut score = 0;
    let mut runs = Runs([0; 7]);
    let (mut color, mut run) = (false, 0);
    for i in 0..SIZE {
        let module = line >> i & 1 == 1;
        if module == color {
            run += 1;
            score += match run {
                5 => 3,
                6.. => 1,
                _ => 0,
            };
            continue;
        }
        runs.push(run);
        if !color {
            score += 40 * runs.finder_like();
        }
        (color, run) = (module, 1);
    }
    if color {
        runs.push(run);
        run = 0;
    }
    runs.push(run + SIZE);
    score + 40 * runs.finder_like()
}

struct Runs([usize; 7]);

impl Runs {
    fn push(&mut self, length: usize) {
        let first = self.0[0] == 0;
        self.0.copy_within(0..6, 1);
        self.0[0] = length + if first { SIZE } else { 0 };
    }

    /// Patterns 1:1:3:1:1 (dark first) with four light modules on either side.
    fn finder_like(&self) -> u32 {
        let r = &self.0;
        let n = r[1];
        let core = n > 0 && r[2] == n && r[3] == 3 * n && r[4] == n && r[5] == n;
        u32::from(core && r[0] >= 4 * n && r[6] >= n)
            + u32::from(core && r[6] >= 4 * n && r[0] >= n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    const PAIR_URL: &[u8] =
        b"vanaride://pair?id=0123abcd&k=AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

    /// From Nayuki's qrcodegen 1.8 (`QrCode.encode_segments([QrSegment.make_bytes(url)],
    /// Ecc.LOW, 4, 4, -1, False)`), which picks mask 6 by the same penalty rules.
    const PAIR_MATRIX: [&str; SIZE] = [
        "#######.#..###.#.#.#.##.#.#######",
        "#.....#..##...#...##.##.#.#.....#",
        "#.###.#...#.##.......##...#.###.#",
        "#.###.#.....#...##..##..#.#.###.#",
        "#.###.#..#.#.#.##.#.......#.###.#",
        "#.....#...#.#.......####..#.....#",
        "#######.#.#.#.#.#.#.#.#.#.#######",
        "........#.##.#..#..##.###........",
        "##.##.#..#######......#.#.#.....#",
        ".###.#.#.#..##...##..#.#.#..#.##.",
        ".####.#.#.###.####...#####.###.##",
        ".##..#.#.####....###....###.#.##.",
        "#.#.####.#...#..##.###.##.#..#.##",
        ".#..##...#.####.#.#....#.#.#...#.",
        ".#.##.##.##.#.###..####..##.###..",
        "..##...###..#...#.##....###..##..",
        ".##...##..#.#....#..##########.#.",
        "..#.##.####....#.####.#...###.#.#",
        ".#..####.####.##.#######..#.###.#",
        "##.###.#....###.#...#.#####..##.#",
        "###..##.#.###.######.#...#.#...#.",
        "###..#.#.##..##.......####.####..",
        "#.....##.#######.##.######.#..###",
        "#.##.#..####..#######...##..###.#",
        "##...##..#.##..#.#.###.######...#",
        "........##.#.##.####...##...#....",
        "#######..##..#.#.#.##.###.#.#....",
        "#.....#...##....#...#...#...#.###",
        "#.###.#.#..#...#.#...#########...",
        "#.###.#.#.#......##.#.#.#..#.....",
        "#.###.#...###..###.#.####...#...#",
        "#.....#.#...##....#...##.####.###",
        "#######.##.#..#..#.##...##.......",
    ];

    fn render(code: &Code) -> Vec<String> {
        (0..SIZE)
            .map(|y| {
                (0..SIZE)
                    .map(|x| if code.dark(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn qr_pair_url_matches_an_independent_encoder() {
        assert_eq!(PAIR_URL.len(), 73);
        let code = encode(PAIR_URL).unwrap();
        assert_eq!(render(&code), PAIR_MATRIX);
    }

    #[test]
    fn qr_rejects_text_past_capacity() {
        assert!(encode(&[b'a'; 78]).is_some());
        assert_eq!(encode(&[b'a'; 79]), None);
    }
}
