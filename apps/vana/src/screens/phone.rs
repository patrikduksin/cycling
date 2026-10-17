//! Connect phone: two QR codes take a phone to its rides with no app.
//!
//! Step 1 joins the device's Wi-Fi (the camera offers to join), step 2
//! opens the page that lists rides and downloads FIT files. The screen moves
//! to step 2 by itself once a phone joins. MOVE (BottomLeft), ACT
//! (BottomRight) or a tap on the code switch steps; BACK (TopLeft) ends the
//! session and returns Home. The runtime prepares the lines and the code;
//! this lays them out.
use super::canvas::{Canvas, Shape};
use super::chrome::{AMBER, Align, BODY, CAPTION, Rect, TITLE, WHITE, WIDTH};
use super::qr::{Placed, Qr};
use super::workout::View;

/// Lines the runtime prepares for this page.
/// What to do now: JOIN VANA WI-FI, OPEN VANA, or why it cannot.
pub const TITLE_LINE: usize = 0;
/// STEP 1 OF 2, or what the radio is doing.
pub const STEP: usize = 1;
/// The network name, or the address to open.
pub const NAME: usize = 2;
/// The password, spaced in fours for reading.
pub const SECRET: usize = 3;
/// A note under the code: who joined, or how the session ends.
pub const NOTE: usize = 4;
/// The title is a problem (amber), not an instruction.
pub const PROBLEM: usize = 5;

const LEFT: usize = 12;
const RIGHT: usize = WIDTH - 12;
const STEP_BASE: usize = 40;
const TITLE_BASE: usize = 64;
/// The code's light panel.
const CODE_TOP: usize = 72;
const CODE_WIDTH: usize = 168;
const RADIUS: usize = 10;
/// A light panel: phone cameras want dark modules on light.
const PAPER: u16 = WHITE;
const INK: u16 = super::chrome::BLACK;

/// Where the code sits for `qr`.
pub fn place(qr: Qr) -> Placed {
    Placed::centred(qr, WIDTH / 2, CODE_TOP, CODE_WIDTH)
}

/// A tap on the code switches steps.
pub fn on_code(x: usize, y: usize) -> bool {
    let panel = Rect {
        left: (WIDTH - CODE_WIDTH) / 2,
        top: CODE_TOP,
        right: (WIDTH + CODE_WIDTH) / 2,
        bottom: CODE_TOP + CODE_WIDTH,
    };
    panel.contains(x, y)
}

pub(super) fn prepare(view: &View) -> Canvas {
    let lines = &view.lines;
    let mut canvas = Canvas::default();
    let width = RIGHT - LEFT;
    canvas.caption(
        lines[STEP].as_bytes(),
        width,
        WIDTH / 2,
        STEP_BASE,
        Align::Center,
    );
    let problem = !lines[PROBLEM].as_bytes().is_empty();
    canvas.fit(
        &[&TITLE, &BODY],
        lines[TITLE_LINE].as_bytes(),
        width,
        WIDTH / 2,
        TITLE_BASE,
        Align::Center,
        if problem { AMBER } else { WHITE },
    );
    let below = match &view.qr {
        Some(code) => {
            canvas.shape(Shape::Bar {
                rect: code.panel(),
                radius: RADIUS,
                color: PAPER,
            });
            code.panel().bottom
        }
        None => TITLE_BASE + 20,
    };
    canvas.fit(
        &[&BODY, &CAPTION],
        lines[NAME].as_bytes(),
        width,
        WIDTH / 2,
        below + 22,
        Align::Center,
        WHITE,
    );
    canvas.fit(
        &[&BODY, &CAPTION],
        lines[SECRET].as_bytes(),
        width,
        WIDTH / 2,
        below + 44,
        Align::Center,
        WHITE,
    );
    canvas.caption(
        lines[NOTE].as_bytes(),
        width,
        WIDTH / 2,
        below + 64,
        Align::Center,
    );
    canvas
}

/// The code's modules over the panel the canvas painted.
pub(super) fn paint(view: &View, row: &mut super::chrome::Row) {
    if let Some(code) = &view.qr {
        code.paint(row, INK);
    }
}

#[cfg(test)]
pub(super) fn pixel(view: &View, x: usize, y: usize) -> Option<u16> {
    let code = view.qr.as_ref()?;
    code.pixel(x, y).map(|dark| if dark { INK } else { PAPER })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{CONTENT, HEIGHT};
    use crate::screens::workout::Page;

    fn view(qr: Option<&str>, problem: bool) -> View<'static> {
        let mut view = View::new(Page::Phone, 0, None);
        let _ = view.lines[STEP].push_str("STEP 1 OF 2");
        let _ = view.lines[TITLE_LINE].push_str(if problem {
            "WI-FI DID NOT START"
        } else {
            "JOIN VANA WI-FI"
        });
        let _ = view.lines[NAME].push_str("VANA-3F2A");
        let _ = view.lines[SECRET].push_str("ABCD EFGH JKMN");
        let _ = view.lines[NOTE].push_str("ENDS AFTER 10 MIN IDLE");
        if problem {
            let _ = view.lines[PROBLEM].push_str("1");
        }
        view.qr = qr.and_then(Qr::encode).map(place);
        view.prepare();
        view
    }

    #[test]
    fn the_code_sits_on_a_light_panel_with_everything_on_screen() {
        let view = view(Some("WIFI:T:WPA;S:VANA-3F2A;P:ABCDEFGHJKMN;;"), false);
        let code = view.qr.unwrap();
        let panel = code.panel();
        assert!(panel.top >= CONTENT.top && panel.right - panel.left <= CODE_WIDTH);
        // Quiet zone is light, the top-left finder is dark.
        assert_eq!(view.pixel(panel.left + 4, panel.top + 4 + RADIUS), PAPER);
        assert_eq!(view.pixel(code.left + 1, code.top + 1), INK);
        view.canvas.assert_laid_out(
            Rect {
                left: 0,
                top: CONTENT.top,
                right: WIDTH,
                bottom: HEIGHT,
            },
            &[
                "STEP 1 OF 2",
                "JOIN VANA WI-FI",
                "VANA-3F2A",
                "ABCD EFGH JKMN",
            ],
            "phone",
        );
        assert!(on_code(WIDTH / 2, CODE_TOP + 40));
        assert!(!on_code(WIDTH / 2, CODE_TOP + CODE_WIDTH + 10));
    }

    #[test]
    fn a_problem_shows_in_amber_without_a_code() {
        let view = view(None, true);
        assert!(view.qr.is_none());
        assert_eq!(
            view.canvas.find("WI-FI DID NOT START").map(|run| run.color),
            Some(AMBER)
        );
    }
}
