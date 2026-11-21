//! Connect phone: two QR codes take a phone to its rides with no app.
//!
//! Step 1 joins the device's Wi-Fi (the camera offers to join), step 2
//! opens the page that lists rides and downloads FIT files. The screen moves
//! to step 2 by itself once a phone joins. MOVE, ACT or a tap on the code
//! switch steps; BACK ends the session and returns Home. The runtime
//! prepares the lines and the code; the Phone screen's tree
//! (`compose::builtin::phone`) lays them out.
use super::chrome::{BLACK, Rect, WHITE};
use super::look::light;
use super::qr::{Placed, Qr};
use super::workout::Scene;

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

/// Readable characters for the access point's password, upper case as
/// the screen shows them: no 0/O or 1/I. It shows in groups of four.
pub const PASSWORD_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const PASSWORD_GROUPS: usize = 3;
pub const GROUP: usize = 4;
/// The access point's name: this, then a group of hex digits.
pub const SSID_PREFIX: &str = "VANA-";
pub const SSID_ALPHABET: &[u8] = b"0123456789ABCDEF";
/// The code's room, square: the most its light panel takes.
pub const CODE_UNITS: usize = 168;

/// The code's light panel's corners, in units.
pub const RADIUS: usize = 10;
/// A light panel: phone cameras want dark modules on light.
const PAPER: u16 = WHITE;
const INK: u16 = BLACK;

/// The palette colour a look shows as `PAPER`.
pub(crate) fn paper(light_look: bool) -> u16 {
    if light_look { light(PAPER) } else { PAPER }
}

/// Where the code sits for `qr` in `room`, the box the Phone screen's tree
/// gives it: centred both ways, its modules as large as the room allows.
pub fn place(room: Rect, qr: Qr) -> Placed {
    let side = room.width().min(room.height());
    let mut placed = Placed::centred(qr, room.center_x(), room.top, side);
    placed.top += (room.height() - placed.panel().height()) / 2;
    placed
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
    use crate::compose::builtin::ScreenId;
    use crate::compose::registry::CODE;
    use crate::screens::chrome::AMBER;
    use crate::screens::workout::{Page, View};
    use crate::screens::{BASE, Layout, panels};

    /// The box the Phone screen's tree gives the code.
    fn room(view: &View) -> Rect {
        let laid = view.laids.and_then(|laids| laids.get(ScreenId::Phone));
        laid.and_then(|laid| laid.box_of(CODE)).unwrap()
    }

    const JOIN: &str = "WIFI:T:WPA;S:VANA-3F2A;P:ABCDEFGHJKMN;;";

    fn view(layout: &'static Layout, qr: Option<&str>, problem: bool) -> View<'static> {
        let mut view = View::new(Page::Phone, 0, None);
        view.layout = layout;
        view.compose_main();
        let room = room(&view);
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
        *code = qr.and_then(Qr::encode).map(|qr| place(room, qr));
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
                let region = room(&view);
                let scene = view.prepare();
                scene.check_layout();
                let code = scene.qr().unwrap();
                let panel = code.panel();
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
            }
        }
    }

    #[test]
    fn the_longest_ended_session_lays_out_whole() {
        crate::screens::check::enable(true);
        for layout in panels::ALL {
            let mut view = View::new(Page::Phone, 0, None);
            view.layout = layout;
            view.compose_main();
            let (_, lines) = view.show_phone();
            let _ = lines[STEP].push_str("PHONE DISCONNECTED");
            let _ = lines[TITLE_LINE].push_str("ENDED AFTER 10 MIN IDLE");
            let _ = lines[NAME].push_str("WAITING FOR WI-FI");
            let _ = lines[NOTE].push_str("TAP TO TRY AGAIN");
            let _ = lines[PROBLEM].push_str("1");
            view.prepare().check_layout();
            let want = [
                "PHONE DISCONNECTED",
                "ENDED AFTER 10 MIN IDLE",
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
