//! What the Phone page shows (`app::phone::page`), and the painter of its
//! pairing code: black modules on the white card the code tile draws, the
//! same in both looks, as a phone's camera reads them best.
use super::chrome::{BLACK, Rect, Row, Shown as Ink};
use crate::compose::builtin::ScreenId;
use firmware_services::qr::{Code, SIZE};

/// Quiet zone in modules: ISO/IEC 18004 asks for four on every side.
pub const QUIET: usize = 4;
pub const MODULE_UNITS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shown<'a> {
    Code(&'a Code),
    Done,
    Paired { connected: bool },
    Forget,
}

impl Shown<'_> {
    pub const fn screen(self) -> ScreenId {
        match self {
            Self::Code(_) => ScreenId::PhoneCode,
            Self::Done => ScreenId::PhoneDone,
            Self::Paired { .. } => ScreenId::PhonePaired,
            Self::Forget => ScreenId::ForgetPhone,
        }
    }
}

pub const fn side(module: usize) -> usize {
    (SIZE + 2 * QUIET) * module
}

pub fn paint(code: &Code, card: Rect, row: &mut Row) {
    let module = card.width() / (SIZE + 2 * QUIET);
    let y = row.y();
    if module == 0 || !(card.top..card.bottom).contains(&y) {
        return;
    }
    let Some(line) = ((y - card.top) / module).checked_sub(QUIET) else {
        return;
    };
    if line >= SIZE {
        return;
    }
    let origin = card.left + QUIET * module;
    let mut x = 0;
    while x < SIZE {
        if !code.dark(x, line) {
            x += 1;
            continue;
        }
        let start = x;
        while x < SIZE && code.dark(x, line) {
            x += 1;
        }
        row.span_shown(origin + start * module..origin + x * module, Ink(BLACK));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::WHITE;

    #[test]
    fn the_painted_modules_are_the_code_s_at_5_px() {
        let code = firmware_services::qr::encode(b"vanaride://pair?id=00000000&k=x").unwrap();
        let module = 5;
        let card = Rect {
            left: 17,
            top: 30,
            right: 17 + side(module),
            bottom: 30 + side(module),
        };
        assert_eq!(card.width(), 205);
        for y in card.top..card.bottom {
            let mut pixels = std::vec![WHITE; 240];
            paint(&code, card, &mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                let inside = |at: usize, from: usize| {
                    let module = (at - from) / module;
                    (QUIET..QUIET + SIZE)
                        .contains(&module)
                        .then(|| module - QUIET)
                };
                let dark = match (x.checked_sub(card.left), card.contains(x, y)) {
                    (Some(_), true) => match (inside(x, card.left), inside(y, card.top)) {
                        (Some(mx), Some(my)) => code.dark(mx, my),
                        _ => false,
                    },
                    _ => false,
                };
                assert_eq!(pixel == BLACK, dark, "{x},{y}");
            }
        }
    }
}
