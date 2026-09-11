//! Export the same renderer used on the device as an RGB image.
use cycling_os::{
    coin::{self, HEIGHT, PIXELS, WIDTH},
    controls::Controls,
};
use std::{
    fs::File,
    io::{self, Write},
};

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "preview.ppm".into());
    let frame = args.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut pixels = [0; PIXELS];
    if args.next().as_deref() == Some("coin") {
        coin::render(frame, &mut pixels);
    } else {
        Controls::default().render(&mut pixels, true);
    }
    let mut out = io::BufWriter::new(File::create(path)?);
    write!(out, "P6\n{} {}\n255\n", WIDTH * 3, HEIGHT * 3)?;
    for y in 0..HEIGHT * 3 {
        for x in 0..WIDTH * 3 {
            let p = pixels[(y / 3) * WIDTH + x / 3];
            out.write_all(&[
                ((p >> 11) * 255 / 31) as u8,
                (((p >> 5) & 63) * 255 / 63) as u8,
                ((p & 31) * 255 / 31) as u8,
            ])?;
        }
    }
    Ok(())
}
