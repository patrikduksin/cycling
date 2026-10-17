//! Connect phone: a placeholder route for the phone companion (rides to the
//! phone over Wi-Fi, FIT export). The phone workstream replaces this screen.
//! Opened from Home it offers every ride; opened from a saved ride, that
//! ride. BACK returns where it came from.
use super::canvas::Canvas;
use super::chrome::{Align, BODY, CAPTION, SECONDARY, TITLE, WHITE, WIDTH};

pub fn prepare(one_ride: bool) -> Canvas {
    let mut canvas = Canvas::default();
    canvas.fit(
        &[&TITLE, &BODY],
        b"PHONE",
        WIDTH - 20,
        WIDTH / 2,
        150,
        Align::Center,
        WHITE,
    );
    canvas.fit(
        &[&CAPTION],
        if one_ride {
            &b"SEND THIS RIDE TO PHONE"[..]
        } else {
            &b"SEND RIDES TO YOUR PHONE"[..]
        },
        WIDTH - 20,
        WIDTH / 2,
        176,
        Align::Center,
        SECONDARY,
    );
    canvas
}
