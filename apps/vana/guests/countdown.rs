//! A ring that fills over each hour of the ride, around the minutes into it.
#![no_std]
mod vana;

const GREEN: i32 = 0x07e0;
const AMBER: i32 = 0xfd20;
const WHITE: i32 = 0xffff;

#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    for face in [vana::LARGE, vana::BODY] {
        let digits = vana::text_width(face, b"59").max(vana::text_height(face));
        let side = digits + 2 * vana::unit(8);
        vana::form(side, side, 1, 1);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn draw(form: i32, width: i32, height: i32) {
    let face = if form == 0 { vana::LARGE } else { vana::BODY };
    let elapsed = vana::metric(vana::ELAPSED);
    let seconds = if elapsed == vana::NONE { 0 } else { elapsed % 3600 };
    let frac = ((seconds as i64) << 16) / 3600;
    let stroke = vana::unit(2).max(1);
    let radius = width.min(height) / 2 - stroke;
    let color = if seconds >= 3300 { AMBER } else { GREEN };
    vana::ring(width / 2, height / 2, radius, stroke, frac as i32, color);
    let mut buffer = [0; 10];
    let minutes = vana::digits((seconds / 60) as u32, &mut buffer);
    let base = (height + vana::text_height(face)) / 2;
    vana::text(face, minutes, base, vana::CENTRE, WHITE);
}
