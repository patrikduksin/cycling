//! Bounded, versioned USB test commands and row compression.
use crate::{companion::Button, input::Point};

#[derive(Default)]
pub struct PointerInjection {
    active: bool,
}

impl PointerInjection {
    /// Returns true when an injected pointer takes ownership from another source.
    pub fn press(&mut self) -> bool {
        let started = !self.active;
        self.active = true;
        started
    }

    /// Returns true only when an injected gesture owned the pointer.
    pub fn finish(&mut self) -> bool {
        core::mem::take(&mut self.active)
    }

    pub fn active(&self) -> bool {
        self.active
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Begin,
    End,
    Ping,
    State,
    Release,
    Cancel,
    Live,
    Wifi,
    Stop,
    Capture,
    Touch(Point),
    Button(Button, u16),
    Battery(u8, u16, u8),
    Record(u16, u8),
}

pub fn parse(line: &str) -> Option<(u32, Action)> {
    let mut words = line.split_ascii_whitespace();
    if words.next()? != "DBG" {
        return None;
    }
    let id = words.next()?.parse().ok()?;
    let verb = words.next()?;
    let mut number = || words.next()?.parse::<u16>().ok();
    let action = match verb {
        "BEGIN" => Action::Begin,
        "END" => Action::End,
        "PING" => Action::Ping,
        "STATE" => Action::State,
        "RELEASE" => Action::Release,
        "CANCEL" => Action::Cancel,
        "LIVE" => Action::Live,
        "WIFI" => Action::Wifi,
        "STOP" => Action::Stop,
        "CAPTURE" => Action::Capture,
        "TOUCH" => {
            let (x, y) = (number()?, number()?);
            if x >= 240 || y >= 320 {
                return None;
            }
            Action::Touch(Point { x, y })
        }
        "BUTTON" => {
            let button = match number()? {
                0 => Button::TopLeft,
                1 => Button::BottomLeft,
                2 => Button::BottomRight,
                _ => return None,
            };
            let code = number()?;
            if !(1..=32767).contains(&code) {
                return None;
            }
            Action::Button(button, code)
        }
        "BATTERY" => {
            let (percent, mv, power) = (number()?, number()?, number()?);
            if percent > 100 || !(2000..=5000).contains(&mv) || power > 2 {
                return None;
            }
            Action::Battery(percent as u8, mv, power as u8)
        }
        "RECORD" => {
            let (ms, fps) = (number()?, number()?);
            if !(100..=30000).contains(&ms) || !(1..=10).contains(&fps) {
                return None;
            }
            Action::Record(ms, fps as u8)
        }
        _ => return None,
    };
    if words.next().is_some() {
        return None;
    }
    Some((id, action))
}

pub struct Lines {
    bytes: [u8; 96],
    len: usize,
    overflow: bool,
}
impl Default for Lines {
    fn default() -> Self {
        Self {
            bytes: [0; 96],
            len: 0,
            overflow: false,
        }
    }
}
impl Lines {
    /// A malformed/oversized line produces an error and never executes a suffix.
    pub fn push(&mut self, byte: u8) -> Option<Result<(u32, Action), ()>> {
        if byte == b'\r' {
            return None;
        }
        if byte == b'\n' {
            let result = if self.overflow {
                None
            } else {
                core::str::from_utf8(&self.bytes[..self.len])
                    .ok()
                    .and_then(parse)
            };
            let legacy = !self.overflow && &self.bytes[..self.len] == b"SCREENSHOT";
            self.len = 0;
            self.overflow = false;
            return if legacy { None } else { Some(result.ok_or(())) };
        }
        if self.len < self.bytes.len() {
            self.bytes[self.len] = byte;
            self.len += 1;
        } else {
            self.overflow = true;
        }
        None
    }
}

/// Hex runs: two digits for count, four for RGB565. At most 480 bytes per row.
pub fn encode_row(row: &[u16], output: &mut [u8; 480]) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let (mut i, mut used) = (0, 0);
    while i < row.len() {
        let mut count = 1;
        while i + count < row.len() && row[i + count] == row[i] && count < 255 {
            count += 1;
        }
        let value = ((count as u32) << 16) | u32::from(row[i]);
        for n in 0..6 {
            output[used + n] = HEX[((value >> (20 - n * 4)) & 15) as usize];
        }
        used += 6;
        i += count;
    }
    used
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_or_ambiguous_commands() {
        for line in [
            "DBG 1 TOUCH 240 0",
            "DBG 1 BATTERY 101 4000 0",
            "DBG 1 RECORD 30001 5",
            "DBG 1 RECORD 1000 0",
            "DBG 1 BUTTON 3 1",
            "DBG 1 STATE extra",
            "DBG 1 BUTTON 0 0",
        ] {
            assert_eq!(parse(line), None, "{line}");
        }
        assert_eq!(
            parse("DBG 12 TOUCH 239 319"),
            Some((12, Action::Touch(Point { x: 239, y: 319 })))
        );
        assert_eq!(parse("DBG 13 CANCEL"), Some((13, Action::Cancel)));
    }
    #[test]
    fn overflow_discards_entire_line_then_recovers() {
        let mut lines = Lines::default();
        for _ in 0..100 {
            assert!(lines.push(b'x').is_none());
        }
        for b in b"DBG 1 BEGIN" {
            assert!(lines.push(*b).is_none());
        }
        assert_eq!(lines.push(b'\n'), Some(Err(())));
        let result = b"DBG 2 STATE\r\n".iter().find_map(|b| lines.push(*b));
        assert_eq!(result, Some(Ok((2, Action::State))));
    }
    #[test]
    fn compresses_runs_without_changing_colors() {
        let mut buffer = [0; 480];
        let n = encode_row(&[0xffff, 0xffff, 0x07e0], &mut buffer);
        assert_eq!(&buffer[..n], b"02ffff0107e0");
        assert_eq!(encode_row(&[0; 80], &mut buffer), 6);
        let row: [u16; 80] = core::array::from_fn(|i| i as u16);
        assert_eq!(encode_row(&row, &mut buffer), 480);
    }

    #[test]
    fn injection_transition_has_explicit_ownership() {
        let mut injection = PointerInjection::default();
        assert!(injection.press());
        assert!(!injection.press());
        assert!(injection.active());
        assert!(injection.finish());
        assert!(!injection.finish());
        assert!(!injection.active());
    }
}
