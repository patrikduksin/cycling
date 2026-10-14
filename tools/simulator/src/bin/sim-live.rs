//! A window onto the simulated device: keys press buttons and change the world.
//! `--frames N --screenshot out.png` renders N frames without a window.
use minifb::{Key, KeyRepeat, Window, WindowOptions};
use shell_simulator::artifacts::Canvas;
use shell_simulator::live::Live;
use std::time::Instant;

const FPS: usize = 30;
const FRAME_MS: f64 = 1000.0 / FPS as f64;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let mut live = Live::new();
    let result = match value("--frames") {
        Some(frames) => {
            let frames: u32 = frames.parse().expect("--frames takes a count");
            headless(&mut live, frames, value("--screenshot"))
        }
        None => window(&mut live),
    };
    if result.is_err() && live.sim.now() == 0 {
        // The window never ran; there is no session worth recording.
        live.sim.discard_recording();
    }
    if std::env::var_os("SIM_ARTIFACTS").is_some() {
        eprintln!("writing the session recording");
    }
    // Dropping the session writes its recording.
    drop(live);
    if let Err(error) = result {
        eprintln!("sim-live: {error}");
        std::process::exit(1);
    }
}

fn headless(live: &mut Live, frames: u32, screenshot: Option<&String>) -> Result<(), String> {
    for _ in 0..frames {
        live.tick(FRAME_MS);
    }
    if let Some(path) = screenshot {
        std::fs::write(path, live.compose().png()).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

fn window(live: &mut Live) -> Result<(), String> {
    let canvas = live.compose();
    let (width, height) = (canvas.width, canvas.height);
    let mut window = Window::new(
        "VANA on simulated C606",
        width,
        height,
        WindowOptions::default(),
    )
    .map_err(|e| e.to_string())?;
    window.set_target_fps(FPS);
    let mut buffer = vec![0u32; width * height];
    let mut last = Instant::now();
    while window.is_open() && !window.is_key_down(Key::Escape) {
        for key in window.get_keys_pressed(KeyRepeat::No) {
            if let Some(c) = char_of(key) {
                live.key_down(c);
            }
        }
        for key in window.get_keys_released() {
            if let Some(c) = char_of(key) {
                live.key_up(c);
            }
        }
        let now = Instant::now();
        live.tick((now - last).as_secs_f64() * 1000.0);
        last = now;
        pack(&live.compose(), &mut buffer);
        window
            .update_with_buffer(&buffer, width, height)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pack(canvas: &Canvas, buffer: &mut [u32]) {
    for (out, &[r, g, b]) in buffer.iter_mut().zip(canvas.rgb.as_chunks::<3>().0) {
        *out = u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b);
    }
}

fn char_of(key: Key) -> Option<char> {
    Some(match key {
        Key::Q => 'q',
        Key::A => 'a',
        Key::D => 'd',
        Key::H => 'h',
        Key::P => 'p',
        Key::C => 'c',
        Key::R => 'r',
        Key::B => 'b',
        Key::G => 'g',
        Key::L => 'l',
        Key::K => 'k',
        Key::V => 'v',
        Key::X => 'x',
        Key::S => 's',
        Key::Space => ' ',
        Key::Key1 => '1',
        Key::Key2 => '2',
        Key::Key3 => '3',
        _ => return None,
    })
}
