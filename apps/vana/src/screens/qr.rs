//! QR codes for the Connect phone screen: encoded once into a module bitmap
//! and painted row by row, dark modules on a light panel so every phone
//! camera reads them.
use super::chrome::{Rect, Row};
use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

/// Version 4 (33 modules) holds 62 bytes at level M: a Wi-Fi join text or
/// the session's URL with room to spare.
const MAX_VERSION: u8 = 4;
const MAX_SIZE: usize = MAX_VERSION as usize * 4 + 17;
/// Light modules around the code, as the standard asks.
pub const QUIET: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Qr {
    size: u8,
    /// One bit per module, bit `x` of row `y`.
    rows: [u64; MAX_SIZE],
}

impl core::fmt::Debug for Qr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Qr({}x{})", self.size, self.size)
    }
}

impl Qr {
    /// The code for `text`, or `None` when it does not fit version 4.
    pub fn encode(text: &str) -> Option<Self> {
        const LEN: usize = Version::new(MAX_VERSION).buffer_len();
        let mut temp = [0u8; LEN];
        let mut out = [0u8; LEN];
        let code = QrCode::encode_text(
            text,
            &mut temp,
            &mut out,
            QrCodeEcc::Medium,
            Version::MIN,
            Version::new(MAX_VERSION),
            None,
            true,
        )
        .ok()?;
        let size = code.size() as usize;
        let mut rows = [0u64; MAX_SIZE];
        for (y, row) in rows.iter_mut().enumerate().take(size) {
            for x in 0..size {
                if code.get_module(x as i32, y as i32) {
                    *row |= 1 << x;
                }
            }
        }
        Some(Self {
            size: size as u8,
            rows,
        })
    }

    pub fn size(&self) -> usize {
        usize::from(self.size)
    }

    pub fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size() && y < self.size() && self.rows[y] >> x & 1 == 1
    }

    /// The largest whole-pixel module scale that fits `width` with the
    /// quiet zone.
    pub fn scale(&self, width: usize) -> usize {
        (width / (self.size() + 2 * QUIET)).max(1)
    }
}

/// A code placed on the screen: `module` pixels per module, its modules
/// starting at (`left`, `top`) inside a light panel `QUIET` modules wider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub qr: Qr,
    pub left: usize,
    pub top: usize,
    pub module: usize,
}

impl Placed {
    /// `qr` centred on `centre_x`, its panel starting at `top`, as large as
    /// fits `width`.
    pub fn centred(qr: Qr, centre_x: usize, top: usize, width: usize) -> Self {
        let module = qr.scale(width);
        let side = qr.size() * module;
        Self {
            qr,
            left: centre_x - side / 2,
            top: top + QUIET * module,
            module,
        }
    }

    /// The light panel, quiet zone included.
    pub fn panel(&self) -> Rect {
        let quiet = QUIET * self.module;
        let side = self.qr.size() * self.module;
        Rect {
            left: self.left - quiet,
            top: self.top - quiet,
            right: self.left + side + quiet,
            bottom: self.top + side + quiet,
        }
    }

    /// Paint the dark modules crossing this row; the panel is a canvas shape.
    pub fn paint(&self, row: &mut Row, dark: u16) {
        let y = row.y();
        let side = self.qr.size() * self.module;
        if !(self.top..self.top + side).contains(&y) {
            return;
        }
        let my = (y - self.top) / self.module;
        for mx in 0..self.qr.size() {
            if self.qr.dark(mx, my) {
                let left = self.left + mx * self.module;
                row.span(left..left + self.module, dark);
            }
        }
    }

    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize) -> Option<bool> {
        let side = self.qr.size() * self.module;
        if !(self.left..self.left + side).contains(&x) || !(self.top..self.top + side).contains(&y)
        {
            return None;
        }
        Some(
            self.qr
                .dark((x - self.left) / self.module, (y - self.top) / self.module),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_text_and_url_fit_and_keep_their_finders() {
        let join = Qr::encode("WIFI:T:WPA;S:VANA-3F2A;P:abcdefghjkmn;;").unwrap();
        let url = Qr::encode("http://192.168.4.1/?t=0123456789abcdef").unwrap();
        for qr in [join, url] {
            assert!(qr.size() <= MAX_SIZE);
            // The three finder patterns: dark corners, a light ring inside.
            let s = qr.size();
            for (x, y) in [(0, 0), (s - 7, 0), (0, s - 7)] {
                assert!(qr.dark(x, y) && qr.dark(x + 6, y + 6) && qr.dark(x + 3, y + 3));
                assert!(!qr.dark(x + 1, y + 1));
            }
        }
        assert!(
            Qr::encode(&"x".repeat(200)).is_none(),
            "too long for version 4"
        );
        let placed = Placed::centred(url, 120, 40, 180);
        let panel = placed.panel();
        assert!(panel.right - panel.left <= 180);
        assert_eq!((panel.left + panel.right) / 2, 120);
    }
}
