//! C606 companion framing and local input reports, recovered from stock N21.

use device_api::input::Button;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Battery {
        percent: u8,
        millivolts: u16,
    },
    /// Stock treats value zero as charging. Other values remain uninterpreted.
    Power {
        status: u8,
    },
    Button {
        button: Button,
        code: u16,
    },
}

/// CRC16/XMODEM over the header and payload; stored little-endian on the wire.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A complete checksum-validated companion envelope.
pub struct Frame {
    bytes: [u8; 256],
    len: usize,
}
impl Frame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    /// Checksum-validated report payload, including variable-length identity pages.
    pub fn report_payload(&self) -> Option<(u8, &[u8])> {
        if self.bytes[4] != 4 {
            return None;
        }
        Some((self.bytes[5], &self.bytes[6..self.len - 2]))
    }
    /// Only the read-only identity query's response envelope. It is separate
    /// from periodic class-four reports and cannot be routed as an ANT page.
    pub fn identity_reply(&self) -> Option<&[u8]> {
        (self.len == 18
            && self.bytes[4] == 5
            && self.bytes[5] == 1
            && self.bytes[6] == 1
            && self.bytes[7] == 1)
            .then_some(&self.bytes[6..16])
    }
    pub fn report(&self) -> Option<(u8, [u8; 8])> {
        if self.len != 16 || self.bytes[4] != 4 {
            return None;
        }
        Some((self.bytes[5], self.bytes[6..14].try_into().ok()?))
    }
    /// Fixed-size command replies, separate from class-four sensor reports.
    pub fn reply(&self) -> Option<(u8, [u8; 8])> {
        if self.len != 16 || self.bytes[4] != 5 {
            return None;
        }
        Some((self.bytes[5], self.bytes[6..14].try_into().ok()?))
    }
    pub fn input(&self) -> Option<Event> {
        decode(self.as_bytes())
    }
}

/// Only the fixed-size command envelope; callers own allowed command semantics.
pub fn command(group: u8, payload: [u8; 8]) -> [u8; 16] {
    let mut frame = [0; 16];
    frame[..6].copy_from_slice(&[0xa5, 12, 0x6f, 0xf1, 2, group]);
    frame[6..14].copy_from_slice(&payload);
    let crc = crc16(&frame[..14]);
    frame[14..].copy_from_slice(&crc.to_le_bytes());
    frame
}

pub struct Decoder {
    bytes: [u8; 256],
    len: usize,
    pub valid_frames: u32,
    pub bad_crc: u32,
}
impl Default for Decoder {
    fn default() -> Self {
        Self {
            bytes: [0; 256],
            len: 0,
            valid_frames: 0,
            bad_crc: 0,
        }
    }
}
impl Decoder {
    pub fn reset(&mut self) {
        self.len = 0;
    }
    fn discard(&mut self, count: usize) {
        self.bytes.copy_within(count..self.len, 0);
        self.len -= count;
    }
    pub fn push(&mut self, byte: u8) -> Option<Event> {
        self.push_frame(byte)
            .and_then(|frame| decode(frame.as_bytes()))
    }
    /// Return validated frames so the single UART owner can route external sensors.
    pub fn push_frame(&mut self, byte: u8) -> Option<Frame> {
        if self.len == self.bytes.len() {
            self.discard(1);
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        loop {
            if self.len == 0 {
                return None;
            }
            if self.bytes[0] != 0xa5 {
                self.discard(1);
                continue;
            }
            if self.len < 4 {
                return None;
            }
            if !(4..=252).contains(&self.bytes[1]) || self.bytes[2..4] != [0x6f, 0xf1] {
                self.discard(1);
                continue;
            }
            let size = usize::from(self.bytes[1]) + 4;
            if self.len < size {
                return None;
            }
            let expected = u16::from_le_bytes([self.bytes[size - 2], self.bytes[size - 1]]);
            if crc16(&self.bytes[..size - 2]) != expected {
                self.bad_crc = self.bad_crc.saturating_add(1);
                self.discard(1);
                continue;
            }
            self.valid_frames = self.valid_frames.saturating_add(1);
            let mut frame = Frame {
                bytes: [0; 256],
                len: size,
            };
            frame.bytes[..size].copy_from_slice(&self.bytes[..size]);
            self.discard(size);
            return Some(frame);
        }
    }
}

/// Reset an already-partial frame before feeding bytes captured after a
/// transport loss. Returns the current loss counter for the caller to retain.
pub fn feed_batch(
    decoder: &mut Decoder,
    previous_losses: u32,
    current_losses: u32,
    bytes: &[u8],
    mut event: impl FnMut(Event),
) -> u32 {
    feed_frames(decoder, previous_losses, current_losses, bytes, |frame| {
        if let Ok(frame) = frame
            && let Some(value) = frame.input()
        {
            event(value);
        }
    })
}

/// Loss is delivered before any frame recovered after a UART or CRC failure.
/// Consumers must invalidate continuity on Err; no corrupt payload is exposed.
pub fn feed_frames(
    decoder: &mut Decoder,
    previous_losses: u32,
    current_losses: u32,
    bytes: &[u8],
    mut event: impl FnMut(Result<Frame, ()>),
) -> u32 {
    if current_losses != previous_losses {
        decoder.reset();
        event(Err(()));
    }
    for byte in bytes {
        let previous_crc = decoder.bad_crc;
        let frame = decoder.push_frame(*byte);
        if decoder.bad_crc != previous_crc {
            event(Err(()));
        }
        if let Some(frame) = frame {
            event(Ok(frame));
        }
    }
    current_losses
}

fn decode(frame: &[u8]) -> Option<Event> {
    // Only the installed companion's eight-byte reports, not external sensors.
    if frame.len() != 16 || frame[4] != 4 {
        return None;
    }
    let p = &frame[6..14];
    match (frame[5], p[0]) {
        (0, 0x52) if p[6] <= 100 => {
            let millivolts = u16::from_le_bytes([p[4], p[5]]);
            if !(2000..=5000).contains(&millivolts) {
                return None;
            }
            Some(Event::Battery {
                percent: p[6],
                millivolts,
            })
        }
        (0x10, 0xf0) if p[1] == 2 => Some(Event::Power { status: p[2] }),
        (0x10, 0x49) => {
            let button = match p[1] {
                0 => Button::TopLeft,
                1 => Button::BottomLeft,
                2 => Button::BottomRight,
                _ => return None,
            };
            let code = u16::from_le_bytes([p[6], p[7]]).checked_sub(0x8000)?;
            if code == 0 {
                return None;
            }
            Some(Event::Button { button, code })
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Default)]
pub struct Status {
    pub battery: Option<(u8, u16)>,
    pub power: Option<u8>,
    pub button_counts: [u32; 3],
    pub last_button: Option<Button>,
}
impl Status {
    pub fn update(&mut self, event: Event) {
        match event {
            Event::Battery {
                percent,
                millivolts,
            } => self.battery = Some((percent, millivolts)),
            Event::Power { status } => self.power = Some(status),
            Event::Button { button, .. } => {
                if let Some(count) = self.button_counts.get_mut(button.index()) {
                    *count = count.saturating_add(1);
                }
                self.last_button = Some(button);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Synthetic packets. Hardware captures stay in ignored .local/.
    fn packet(group: u8, payload: [u8; 8]) -> [u8; 16] {
        let mut f = [0; 16];
        f[..6].copy_from_slice(&[0xa5, 12, 0x6f, 0xf1, 4, group]);
        f[6..14].copy_from_slice(&payload);
        let crc = crc16(&f[..14]);
        f[14..].copy_from_slice(&crc.to_le_bytes());
        f
    }
    #[test]
    fn standard_crc_vector() {
        assert_eq!(crc16(b"123456789"), 0x31c3);
    }
    #[test]
    fn unknown_button_codes_remain_counted_for_investigation() {
        let mut status = Status::default();
        status.update(Event::Button {
            button: Button::TopLeft,
            code: 2,
        });
        assert_eq!(status.button_counts, [1, 0, 0]);
        assert_eq!(status.last_button, Some(Button::TopLeft));
    }
    #[test]
    fn fragmented_battery_and_button_reports() {
        let f = packet(0, [0x52, 0xff, 0xff, 0xff, 0xa0, 0x0f, 73, 0xff]);
        let mut d = Decoder::default();
        for &b in &f[..15] {
            assert_eq!(d.push(b), None);
        }
        assert_eq!(
            d.push(f[15]),
            Some(Event::Battery {
                percent: 73,
                millivolts: 4000
            })
        );
        for id in 0..3 {
            let f = packet(16, [0x49, id, 0, 0, 0, 0, 1, 0x80]);
            let event = f.into_iter().find_map(|b| d.push(b)).unwrap();
            assert_eq!(
                event,
                Event::Button {
                    button: [Button::TopLeft, Button::BottomLeft, Button::BottomRight][id as usize],
                    code: 1
                }
            );
        }
        assert_eq!(d.valid_frames, 4);
    }
    #[test]
    fn corrupt_frame_resynchronizes_without_false_button() {
        let good = packet(16, [0x49, 0, 0, 0, 0, 0, 1, 0x80]);
        let mut bad = good;
        bad[9] ^= 1;
        let mut d = Decoder::default();
        for b in [0, 0xa5, 0, 0, 0].into_iter().chain(bad) {
            assert_eq!(d.push(b), None);
        }
        assert_eq!(
            good.into_iter().find_map(|b| d.push(b)),
            Some(Event::Button {
                button: Button::TopLeft,
                code: 1
            })
        );
        assert_eq!(d.bad_crc, 1);
    }
    #[test]
    fn reported_loss_resets_partial_before_post_loss_batch() {
        let old = packet(16, [0x49, 0, 0, 0, 0, 0, 1, 0x80]);
        let fresh = packet(0, [0x52, 0xff, 0xff, 0xff, 0xa0, 0x0f, 73, 0xff]);
        let mut decoder = Decoder::default();
        for byte in &old[..8] {
            assert_eq!(decoder.push(*byte), None);
        }
        let mut events = [None; 1];
        let losses = feed_batch(&mut decoder, 0, 1, &fresh, |event| events[0] = Some(event));
        assert_eq!(losses, 1);
        assert_eq!(
            events[0],
            Some(Event::Battery {
                percent: 73,
                millivolts: 4000
            })
        );
        assert_eq!((decoder.valid_frames, decoder.bad_crc), (1, 0));
    }
    #[test]
    fn rejects_invalid_fields_and_unrelated_reports() {
        for f in [
            packet(0, [0x52, 0, 0, 0, 0xa0, 0x0f, 101, 0]),
            packet(0, [0x52, 0, 0, 0, 0, 0, 50, 0]),
            packet(16, [0x49, 3, 0, 0, 0, 0, 1, 0x80]),
            packet(16, [0x49, 0, 0, 0, 0, 0, 1, 0]),
            packet(16, [0xf1, 1, 0, 0, 0, 0, 0, 0]),
        ] {
            let mut d = Decoder::default();
            assert!(f.into_iter().all(|b| d.push(b).is_none()));
        }
    }
    #[test]
    fn power_keeps_unknown_states_and_transport_reset_drops_partial() {
        let f = packet(16, [0xf0, 2, 7, 0, 0, 0, 0, 0]);
        let mut d = Decoder::default();
        for &b in &f[..9] {
            d.push(b);
        }
        d.reset();
        assert_eq!(
            f.into_iter().find_map(|b| d.push(b)),
            Some(Event::Power { status: 7 })
        );
    }
}
