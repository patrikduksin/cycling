//! The back-to-start page: an arrow to ride by, how far the start is
//! straight and back along the ride, and a pad that follows the ride's own
//! track back.
//!
//! The arrow points relative to the way the rider rides, so straight up is
//! straight on; without a heading (stopped) it points north up and says so.
//! While following, it points along the track a little way ahead, or back to
//! the track when off it. `prepare` lays out the text, the compass ring and
//! the pad; `frame` places the arrow once per frame and `paint_arrow` fills
//! its rows, anti-aliased, over them.
use super::{LEFT, RIGHT, cell, kilometres};
use crate::ride::homing::{Guide, Mode, TURN};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, BODY, CAPTION, GREEN, RAISED, Rect, Ring, Row, SECONDARY, TERTIARY, WHITE,
    on_circle,
};
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::blend;

/// What the page knows of the way back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Back {
    /// The device has no memory set aside for the track.
    #[default]
    Unavailable,
    /// No recent fix; following the track back carries on regardless.
    Waiting {
        following: bool,
    },
    Guide(Guide),
}

/// The title and the state share this baseline.
const HEAD: usize = 40;
/// The compass: a ring around the arrow, in pixels.
const CX: usize = (LEFT + RIGHT) / 2;
const CY: usize = 110;
const RING_RADIUS: usize = 54;
const RING_WIDTH: usize = 3;
/// The arrow's reach from the centre, tip to tail.
const ARROW_RADIUS: f32 = 40.0;
/// What the arrow shows, under the compass.
const STATE: usize = CY + RING_RADIUS + 16;
/// The distances under it.
const CELLS: usize = STATE + 4;
/// The pad that follows the track back, above the toast band.
pub const PAD: Rect = Rect {
    left: LEFT,
    top: CELLS + 52,
    right: RIGHT,
    bottom: CELLS + 52 + 38,
};
const _: () = assert!(PAD.bottom < crate::screens::chrome::TOAST.top);
const _: () = assert!(HEAD + 12 < CY - RING_RADIUS - 2);

/// The arrow: a navigation chevron as two triangles that share its spine,
/// in pixels, and its colour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Arrow {
    parts: [[(f32, f32); 3]; 2],
    color: u16,
    shown: bool,
}

/// The chevron pointing up, in units of `ARROW_RADIUS`: tip, wing, notch.
const TIP: (f32, f32) = (0.0, -1.0);
const WING: (f32, f32) = (0.66, 0.85);
const NOTCH: (f32, f32) = (0.0, 0.45);

fn colour(mode: Mode) -> u16 {
    match mode {
        Mode::Direct => WHITE,
        Mode::Track => GREEN,
        Mode::OffTrack => AMBER,
        Mode::Arrived => GREEN,
    }
}

/// The arrow for this frame, if the page shows one.
pub(super) fn frame(view: &View) -> Arrow {
    let Back::Guide(guide) = view.back else {
        return Arrow::default();
    };
    if guide.mode == Mode::Arrived {
        return Arrow::default();
    }
    // Rotate by the arrow's angle with the ring's own sine, clockwise.
    let angle = guide.arrow();
    let unit = 4096;
    let (sx, sy) = on_circle(0, 0, unit, angle);
    let (sin, cos) = (sx as f32 / unit as f32, -sy as f32 / unit as f32);
    let place = |(x, y): (f32, f32)| {
        let (x, y) = (x * ARROW_RADIUS, y * ARROW_RADIUS);
        (CX as f32 + x * cos - y * sin, CY as f32 + x * sin + y * cos)
    };
    let wing_left = (-WING.0, WING.1);
    Arrow {
        parts: [
            [place(TIP), place(WING), place(NOTCH)],
            [place(TIP), place(NOTCH), place(wing_left)],
        ],
        color: colour(guide.mode),
        shown: true,
    }
}

/// Where row `y` crosses a triangle: its left and right edges, if at all.
fn crossing(part: &[(f32, f32); 3], y: f32) -> Option<(f32, f32)> {
    let mut span: Option<(f32, f32)> = None;
    for i in 0..3 {
        let (a, b) = (part[i], part[(i + 1) % 3]);
        let (low, high) = if a.1 <= b.1 { (a, b) } else { (b, a) };
        if y < low.1 || y >= high.1 {
            continue;
        }
        let x = low.0 + (y - low.1) / (high.1 - low.1) * (high.0 - low.0);
        span = Some(span.map_or((x, x), |(l, r)| (l.min(x), r.max(x))));
    }
    span
}

/// Four sample rows per pixel row; each spans the triangles it crosses.
type Spans = [[Option<(f32, f32)>; 2]; 4];

fn spans(arrow: &Arrow, y: usize) -> Spans {
    core::array::from_fn(|sub| {
        let y = y as f32 + (sub as f32 + 0.5) / 4.0;
        core::array::from_fn(|part| crossing(&arrow.parts[part], y))
    })
}

/// How much of pixel `x` the arrow covers, 0 to 15.
fn coverage(spans: &Spans, x: usize) -> u8 {
    let (left, right) = (x as f32, x as f32 + 1.0);
    let mut covered = 0.0;
    for sub in spans {
        for &(l, r) in sub.iter().flatten() {
            covered += (r.min(right) - l.max(left)).clamp(0.0, 1.0);
        }
    }
    ((covered * 15.0 / 4.0 + 0.5) as u8).min(15)
}

/// The columns the arrow can touch.
const REACH: usize = ARROW_RADIUS as usize + 2;

/// The arrow's rows of the page, placed from the guide as they paint: the
/// view keeps no arrow, so the frame costs no stack for it.
pub(super) fn paint(view: &View, row: &mut Row) {
    let y = row.y();
    if y + REACH >= CY && y <= CY + REACH {
        paint_arrow(&frame(view), row);
    }
}

/// Fill the arrow's part of a row over what the row holds.
pub(super) fn paint_arrow(arrow: &Arrow, row: &mut Row) {
    let y = row.y();
    if !arrow.shown || y + REACH < CY || y > CY + REACH {
        return;
    }
    let spans = spans(arrow, y);
    if spans.iter().flatten().all(Option::is_none) {
        return;
    }
    row.each_over(CX - REACH..CX + REACH, |x, under| {
        match coverage(&spans, x) {
            0 => None,
            c => Some(blend(arrow.color, under, c)),
        }
    });
}

/// The per-pixel original of `paint_arrow`.
#[cfg(test)]
pub(super) fn arrow_pixel(arrow: &Arrow, x: usize, y: usize, under: u16) -> u16 {
    if !arrow.shown || x + REACH < CX || x >= CX + REACH {
        return under;
    }
    match coverage(&spans(arrow, y), x) {
        0 => under,
        c => blend(arrow.color, under, c),
    }
}

fn state(back: Back) -> (&'static [u8], u16) {
    match back {
        Back::Unavailable => (b"NO TRACK", SECONDARY),
        Back::Waiting { .. } => (b"NO GPS", AMBER),
        Back::Guide(guide) => match guide.mode {
            Mode::Direct => (b"DIRECT", SECONDARY),
            Mode::Track => (b"ON TRACK", GREEN),
            Mode::OffTrack => (b"OFF TRACK", AMBER),
            Mode::Arrived => (b"AT START", GREEN),
        },
    }
}

/// The text, the compass ring and the pad; the arrow is `frame`'s.
pub(super) fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let width = RIGHT - LEFT;
    canvas.caption(b"BACK TO START", width, LEFT, HEAD, Align::Left);
    let guide = match view.back {
        Back::Guide(guide) => Some(guide),
        _ => None,
    };
    let (status, status_color) = state(view.back);
    let mut line = Text::<24>::new();
    let _ = line.push_str(core::str::from_utf8(status).unwrap_or(""));
    // Without a heading the arrow points north up, and says so.
    if guide.is_some_and(|guide| guide.heading.is_none() && guide.mode != Mode::Arrived) {
        let _ = line.push_str("  N UP");
    }
    canvas.fit(
        &[&CAPTION],
        line.as_bytes(),
        width,
        CX,
        STATE,
        Align::Center,
        status_color,
    );
    let ring_color = match guide {
        Some(guide) if guide.mode == Mode::Arrived => GREEN,
        Some(guide) if guide.mode == Mode::OffTrack => AMBER,
        _ => TERTIARY,
    };
    canvas.shape(Shape::ring(
        Ring {
            cx: 4 * CX as i32 + 2,
            cy: 4 * CY as i32 + 2,
            radius: 4 * RING_RADIUS as i32,
            width: 4 * RING_WIDTH as i32,
            start: 0,
            sweep: TURN,
        },
        ring_color,
    ));
    // Inside the ring: what the arrow cannot say.
    let inside = 2 * (RING_RADIUS - 8);
    let middle = |canvas: &mut Canvas, text: &[u8], base: usize, color: u16| {
        canvas.fit(
            &[&BODY, &CAPTION],
            text,
            inside,
            CX,
            base,
            Align::Center,
            color,
        );
    };
    match guide {
        None => middle(&mut canvas, b"--", CY + 6, SECONDARY),
        Some(guide) if guide.mode == Mode::Arrived => {
            middle(&mut canvas, b"HOME", CY + 6, GREEN);
        }
        Some(_) => {}
    }

    let mut values: [Text<12>; 2] = core::array::from_fn(|_| Text::new());
    let unknown = |value: &mut Text<12>| {
        let _ = value.push_str("--");
    };
    match guide {
        Some(guide) => {
            kilometres(&mut values[0], u64::from(guide.direct_m) * 1000);
            kilometres(&mut values[1], u64::from(guide.track_m) * 1000);
        }
        None => values.iter_mut().for_each(unknown),
    }
    cell(
        &mut canvas,
        0,
        CELLS,
        b"DIRECT KM",
        values[0].as_bytes(),
        WHITE,
    );
    cell(
        &mut canvas,
        1,
        CELLS,
        b"TRACK KM",
        values[1].as_bytes(),
        WHITE,
    );

    let following = match view.back {
        Back::Guide(guide) => guide.following,
        Back::Waiting { following } => following,
        Back::Unavailable => false,
    };
    canvas.shape(Shape::Panel {
        rect: PAD,
        radius: 12,
        color: RAISED,
    });
    let pad: &[u8] = if following {
        b"STOP FOLLOWING"
    } else {
        b"FOLLOW TRACK BACK"
    };
    canvas.fit(
        &[&BODY, &CAPTION],
        pad,
        PAD.right - PAD.left - 24,
        CX,
        PAD.top + 26,
        Align::Center,
        if following { WHITE } else { GREEN },
    );
    canvas
}

/// Whether a tap at (`x`, `y`) lands on the pad, where the page sits beside
/// a radar strip; `centred` when the page is shifted to the middle.
pub fn pad_at(x: usize, y: usize, centred: bool) -> bool {
    let x = if centred { x + super::CENTRE_SHIFT } else { x };
    (PAD.left..PAD.right).contains(&x) && (PAD.top..PAD.bottom).contains(&y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::ride::{BODY as PAGE, RidePage};
    use crate::screens::workout::Page;

    fn guide(mode: Mode, heading: Option<u16>, bearing: u16) -> Guide {
        Guide {
            mode,
            following: mode != Mode::Direct,
            direct_m: 1_234,
            track_m: 2_480,
            off_m: 0,
            bearing,
            heading,
        }
    }

    fn page(back: Back) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Back;
        view.radar_paired = true;
        view.back = back;
        view.prepare();
        view
    }

    #[test]
    fn every_state_lays_out_its_text() {
        let cases: [(Back, &[&str]); 6] = [
            (
                Back::Unavailable,
                &["BACK TO START", "NO TRACK", "FOLLOW TRACK BACK"],
            ),
            (
                Back::Waiting { following: false },
                &["NO GPS", "DIRECT KM", "--"],
            ),
            (
                Back::Waiting { following: true },
                &["NO GPS", "STOP FOLLOWING"],
            ),
            (
                Back::Guide(guide(Mode::Direct, None, 1024)),
                &["DIRECT  N UP", "1.23", "2.48", "FOLLOW TRACK BACK"],
            ),
            (
                Back::Guide(guide(Mode::OffTrack, Some(0), 2048)),
                &["OFF TRACK", "TRACK KM", "STOP FOLLOWING"],
            ),
            (
                Back::Guide(guide(Mode::Arrived, Some(0), 0)),
                &["AT START", "HOME", "STOP FOLLOWING"],
            ),
        ];
        for (back, want) in cases {
            let view = page(back);
            view.canvas
                .assert_laid_out(PAGE, want, &std::format!("{back:?}"));
        }
    }

    #[test]
    fn the_arrow_turns_with_the_heading() {
        // The start due east, riding north: the arrow points right.
        let view = page(Back::Guide(guide(Mode::Track, Some(0), 1024)));
        let tip = frame(&view).parts[0][0];
        assert!((tip.0 - (CX as f32 + ARROW_RADIUS)).abs() < 0.5, "{tip:?}");
        assert!((tip.1 - CY as f32).abs() < 0.5, "{tip:?}");
        // Riding east too: straight up.
        let view = page(Back::Guide(guide(Mode::Track, Some(1024), 1024)));
        let tip = frame(&view).parts[0][0];
        assert!((tip.0 - CX as f32).abs() < 0.5, "{tip:?}");
        assert!((tip.1 - (CY as f32 - ARROW_RADIUS)).abs() < 0.5, "{tip:?}");
        assert_eq!(frame(&view).color, GREEN);
        // The arrow is solid inside and soft at its edges.
        let spans = spans(&frame(&view), CY);
        assert_eq!(coverage(&spans, CX), 15);
        assert_eq!(coverage(&spans, CX + REACH - 1), 0);
        // No arrow at the start, or without a fix.
        assert!(!frame(&page(Back::Guide(guide(Mode::Arrived, None, 0)))).shown);
        assert!(!frame(&page(Back::Waiting { following: true })).shown);
    }

    #[test]
    fn taps_find_the_pad_beside_a_radar_or_centred() {
        let (x, y) = ((PAD.left + PAD.right) / 2, PAD.top + 10);
        assert!(pad_at(x, y, false));
        assert!(pad_at(x - super::super::CENTRE_SHIFT, y, true));
        assert!(!pad_at(x, PAD.top - 2, false));
        assert!(!pad_at(x, PAD.bottom, false));
    }
}
