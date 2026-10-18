//! Holding TopLeft (BACK, also the power button) reboots the C606 whatever the
//! firmware is doing: the rider's way out of a state nothing else leaves, such
//! as a dark, lit panel. Input acquisition watches every report, before shell,
//! VANA or the power owner can drop it.
use device_api::input::Button;

pub const BUTTON: Button = Button::TopLeft;
/// Far longer than any press or hold the interface uses.
pub const HOLD_MS: u64 = 10_000;
/// Observed companion codes: 1 for a click; while a button is held, 4
/// repeated about every 250 ms; 5 when it is let go.
const HELD: u16 = 4;
/// A gap this long between held reports means the release report was lost.
const GAP_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct Hold {
    /// The first and latest held report of the current hold.
    reports: Option<(u64, u64)>,
}

impl Hold {
    /// Note one button report; returns whether the hold has lasted `HOLD_MS`.
    pub fn report(&mut self, button: Button, code: u16, now: u64) -> bool {
        if button != BUTTON {
            return false;
        }
        if code != HELD {
            self.reports = None;
            return false;
        }
        let since = match self.reports {
            Some((since, last)) if now.saturating_sub(last) < GAP_MS => since,
            _ => now,
        };
        self.reports = Some((since, now));
        now.saturating_sub(since) >= HOLD_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(hold: &mut Hold, from: u64, until: u64) -> bool {
        (from..=until)
            .step_by(250)
            .any(|now| hold.report(BUTTON, HELD, now))
    }

    #[test]
    fn ten_second_hold_reboots() {
        let mut h = Hold::default();
        assert!(!hold(&mut h, 0, HOLD_MS - 250));
        assert!(h.report(BUTTON, HELD, HOLD_MS));
    }

    #[test]
    fn release_or_a_long_gap_starts_over() {
        let mut h = Hold::default();
        assert!(!hold(&mut h, 0, 9_000));
        assert!(!h.report(BUTTON, 5, 9_100));
        assert!(!hold(&mut h, 9_200, 9_200 + HOLD_MS - 250));
        let mut h = Hold::default();
        assert!(!hold(&mut h, 0, 9_000));
        assert!(!hold(
            &mut h,
            9_000 + GAP_MS,
            9_000 + GAP_MS + HOLD_MS - 250
        ));
    }

    #[test]
    fn clicks_and_other_buttons_do_not_count() {
        let mut h = Hold::default();
        for now in (0..=2 * HOLD_MS).step_by(250) {
            assert!(!h.report(Button::BottomLeft, HELD, now));
            assert!(!h.report(Button::BottomRight, HELD, now));
            assert!(!h.report(BUTTON, 1, now));
        }
    }
}
