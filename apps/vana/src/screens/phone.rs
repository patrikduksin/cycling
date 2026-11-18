//! Connect phone: two QR codes take a phone to its rides with no app.
//!
//! Step 1 joins the device's Wi-Fi (the camera offers to join), step 2
//! opens the page that lists rides and downloads FIT files. The screen moves
//! to step 2 by itself once a phone joins. MOVE (BottomLeft), ACT
//! (BottomRight) or a tap on the code switch steps; BACK (TopLeft) ends the
//! session and returns Home. The runtime prepares the lines and the code;
//! this lays them out.
use super::Layout;
use super::canvas::{Canvas, Shape};
use super::chrome::{AMBER, Align, BLACK, Rect, WHITE};
use super::look::light;
use super::qr::{Placed, Qr};
use super::scaled::{Scaled, Track};
use super::workout::{Scene, View};
use firmware_shell::rendering::font::FaceRole;

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

/// Connect phone on one panel, under the status bar: the step and the title,
/// their capitals ending on their rows' bottoms, then the code. The lines
/// under the code follow its light panel, whose size the code sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub step: Rect,
    pub title: Rect,
    /// The most the code's light panel takes, and where a tap switches steps.
    pub code: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let content = super::chrome::regions(at).content;
    let [_, text, _] = content.cols([Unit(12), Fr(1), Unit(12)], Unit(0), at);
    let [_, square, _] = content.cols([Fr(1), Unit(168), Fr(1)], Unit(0), at);
    let [step, title, _, code, _] =
        content.rows([Unit(16), Unit(24), Unit(8), Unit(168), Fr(1)], Unit(0), at);
    Regions {
        step: step.with_columns(text),
        title: title.with_columns(text),
        code: code.with_columns(square),
    }
}

/// The code's light panel's corners, in units.
pub const RADIUS: usize = 10;
/// A light panel: phone cameras want dark modules on light.
const PAPER: u16 = WHITE;
const INK: u16 = BLACK;

/// The palette colour a look shows as `PAPER`.
fn paper(light_look: bool) -> u16 {
    if light_look { light(PAPER) } else { PAPER }
}

/// Where the code sits for `qr` on `layout`'s panel: centred in the code's
/// region, its modules as large as the region's width allows.
pub fn place(layout: &Layout, qr: Qr) -> Placed {
    let code = layout.phone.code;
    Placed::centred(qr, code.center_x(), code.top, code.width())
}

/// A tap at panel point (`x`, `y`) on the code switches steps.
pub fn on_code(layout: &Layout, x: usize, y: usize) -> bool {
    layout.phone.code.contains(x, y)
}

pub(super) fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.phone);
    let [title, body, caption] =
        [FaceRole::Title, FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    let lines = &view.lines();
    let mut canvas = Canvas::default();
    let (width, centre) = (regions.title.width(), regions.title.center_x());
    let step = regions.step;
    canvas.caption(
        at,
        lines[STEP].as_bytes(),
        width,
        centre,
        step.bottom,
        Align::Center,
    );
    let problem = !lines[PROBLEM].as_bytes().is_empty();
    canvas.fit(
        &[title, body],
        lines[TITLE_LINE].as_bytes(),
        width,
        centre,
        regions.title.bottom,
        Align::Center,
        if problem { AMBER } else { WHITE },
    );
    let below = match view.qr() {
        Some(code) => {
            canvas.shape(Shape::Bar {
                rect: code.panel(),
                radius: at.unit(RADIUS),
                color: paper(view.light),
            });
            code.panel().bottom
        }
        None => regions.title.bottom + at.unit(20),
    };
    let mut line = |text: &[u8], offset| {
        let base = below + at.unit(offset);
        canvas.fit(
            &[body, caption],
            text,
            width,
            centre,
            base,
            Align::Center,
            WHITE,
        );
    };
    line(lines[NAME].as_bytes(), 22);
    line(lines[SECRET].as_bytes(), 44);
    let base = below + at.unit(64);
    canvas.caption(
        at,
        lines[NOTE].as_bytes(),
        width,
        centre,
        base,
        Align::Center,
    );
    canvas
}

/// The code's modules over the panel the canvas painted, shown as `INK` in
/// every look: the light look shows `light(INK)` as `INK`.
pub(super) fn paint(scene: &Scene, row: &mut super::chrome::Row) {
    if let Some(code) = scene.qr() {
        let ink = if row.ink(INK).0 == INK {
            INK
        } else {
            light(INK)
        };
        code.paint(row, ink);
    }
}

#[cfg(test)]
pub(super) fn pixel(scene: &Scene, x: usize, y: usize) -> Option<super::chrome::Shown> {
    let code = scene.qr()?;
    code.pixel(x, y)
        .map(|dark| super::chrome::Shown(if dark { INK } else { PAPER }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;
    use crate::screens::{BASE, panels};

    const JOIN: &str = "WIFI:T:WPA;S:VANA-3F2A;P:ABCDEFGHJKMN;;";

    fn view(layout: &'static Layout, qr: Option<&str>, problem: bool) -> View<'static> {
        let mut view = View::new(Page::Phone, 0, None);
        view.layout = layout;
        let (code, lines) = view.show_phone();
        let _ = lines[STEP].push_str("STEP 1 OF 2");
        let _ = lines[TITLE_LINE].push_str(if problem {
            "WI-FI DID NOT START"
        } else {
            "JOIN VANA WI-FI"
        });
        let _ = lines[NAME].push_str("VANA-3F2A");
        let _ = lines[SECRET].push_str("ABCD EFGH JKMN");
        let _ = lines[NOTE].push_str("ENDS AFTER 10 MIN IDLE");
        if problem {
            let _ = lines[PROBLEM].push_str("1");
        }
        *code = qr.and_then(Qr::encode).map(|qr| place(layout, qr));
        view.prepare();
        view
    }

    #[test]
    fn the_code_sits_on_a_light_panel_with_everything_on_screen() {
        crate::screens::check::enable(true);
        let largest = "http://192.168.4.1/?t=0123456789abcdef0123456789abcdef";
        assert_eq!(Qr::encode(largest).map(|qr| qr.size()), Some(33));
        for layout in panels::ALL {
            for text in [JOIN, largest] {
                let mut view = view(layout, Some(text), false);
                let scene = view.prepare();
                scene.check_layout();
                let code = scene.qr().unwrap();
                let panel = code.panel();
                let region = layout.phone.code;
                let what = std::format!("{text} on {:?}", layout.scaled.panel);
                assert!(crate::screens::check::inside(panel, region), "{what}");
                let radius = layout.scaled.unit(RADIUS);
                assert_eq!(scene.pixel(panel.left + 2, panel.top + 2 + radius), PAPER);
                assert_eq!(scene.pixel(code.left + 1, code.top + 1), INK);
                let page = Rect {
                    bottom: layout.scaled.panel.height,
                    ..layout.chrome.content
                };
                let want = [
                    "STEP 1 OF 2",
                    "JOIN VANA WI-FI",
                    "VANA-3F2A",
                    "ABCD EFGH JKMN",
                    "ENDS AFTER 10 MIN IDLE",
                ];
                scene.canvas().assert_laid_out(page, &want, &what);
                assert!(on_code(layout, region.center_x(), region.top + 4));
                assert!(!on_code(layout, region.center_x(), region.bottom + 1));
            }
        }
    }

    #[test]
    fn the_longest_ended_session_lays_out_whole() {
        crate::screens::check::enable(true);
        for layout in panels::ALL {
            let mut view = View::new(Page::Phone, 0, None);
            view.layout = layout;
            let (_, lines) = view.show_phone();
            let _ = lines[STEP].push_str("PHONE DISCONNECTED");
            let _ = lines[TITLE_LINE].push_str("ENDED AFTER 10 MIN IDLE");
            let _ = lines[NAME].push_str("WAITING FOR WI-FI");
            let _ = lines[NOTE].push_str("TAP TO TRY AGAIN");
            let _ = lines[PROBLEM].push_str("1");
            view.prepare().check_layout();
            let want = [
                "PHONE DISCONNECTED",
                "WAITING FOR WI-FI",
                "TAP TO TRY AGAIN",
            ];
            let what = std::format!("ended on {:?}", layout.scaled.panel);
            view.canvas()
                .assert_laid_out(layout.scaled.screen(), &want, &what);
        }
    }

    #[test]
    fn the_light_look_keeps_dark_modules_on_a_light_panel() {
        let mut view = view(&BASE, Some(JOIN), false);
        view.light = true;
        let scene = view.prepare();
        let code = scene.qr().unwrap();
        let panel = code.panel();
        let shown = |x: usize, y: usize| scene.pixel(x, y);
        // The quiet zone, the panel's corner and the page around it are
        // light; the top-left finder's corner module is dark.
        assert_eq!(shown(panel.left + 4, panel.top + 4 + RADIUS), WHITE);
        assert_eq!(shown(panel.left, panel.top), WHITE);
        assert_eq!(shown(panel.left - 2, panel.top + 20), WHITE);
        assert_eq!(shown(code.left + 1, code.top + 1), BLACK);
    }

    #[test]
    fn a_problem_shows_in_amber_without_a_code() {
        let view = view(&BASE, None, true);
        assert!(view.qr().is_none());
        assert_eq!(
            view.canvas()
                .find("WI-FI DID NOT START")
                .map(|run| run.color),
            Some(AMBER)
        );
    }
}
