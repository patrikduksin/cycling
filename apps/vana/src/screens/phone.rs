//! Connect phone: a placeholder route for the phone companion (rides to the
//! phone over Wi-Fi, FIT export). The phone workstream replaces this screen;
//! BACK returns Home.
use super::canvas::Canvas;
use super::chrome::{Align, BODY, CAPTION, SECONDARY, TITLE, WHITE, WIDTH};

pub fn prepare() -> Canvas {
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
        b"SEND RIDES TO YOUR PHONE",
        WIDTH - 20,
        WIDTH / 2,
        176,
        Align::Center,
        SECONDARY,
    );
    canvas
}
