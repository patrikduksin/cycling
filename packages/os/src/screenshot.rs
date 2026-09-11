//! USB screenshot framing. Each row is an atomic log line.
use crate::coin::{HEIGHT, PIXELS, WIDTH};

pub fn checksum(canvas: &[u16; PIXELS]) -> u32 {
    canvas
        .iter()
        .flat_map(|p| p.to_le_bytes())
        .fold(0x811c9dc5u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
        })
}

#[derive(Default)]
pub struct Command {
    matched: usize,
}

impl Command {
    pub fn push(&mut self, byte: u8) -> bool {
        const COMMAND: &[u8] = b"SCREENSHOT\n";
        if byte == b'\r' {
            return false;
        }
        if byte == COMMAND[self.matched] {
            self.matched += 1;
            if self.matched == COMMAND.len() {
                self.matched = 0;
                return true;
            }
        } else {
            self.matched = usize::from(byte == COMMAND[0]);
        }
        false
    }
}

/// The same nearest-neighbor mapping used by the LCD DMA strips.
pub fn canvas_index(x: usize, y: usize) -> usize {
    ((y.saturating_sub(1) / 3).min(HEIGHT - 1)) * WIDTH + x / 3
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_recover_from_noise_and_repeat() {
        let mut command = Command::default();
        let count = b"garbage\nSCREESCREENSHOT\r\nSCREENSHOT\n"
            .iter()
            .filter(|&&b| command.push(b))
            .count();
        assert_eq!(count, 2);
    }
    #[test]
    fn physical_edges_match_canvas() {
        assert_eq!(canvas_index(0, 0), 0);
        assert_eq!(canvas_index(239, 319), PIXELS - 1);
        assert_eq!(canvas_index(0, 3), 0);
        assert_eq!(canvas_index(0, 4), WIDTH);
    }
}
