use device_api::input::Button;
use firmware_shell::rendering::record::{self, Kind};
use shell_simulator::bench::SCENES;
use shell_simulator::sim::{Sim, seconds};
use vana::screens::look::light;
use vana::screens::phone;
use vana::screens::workout::Page;

fn reach(name: &str) -> Sim {
    let scene = SCENES.iter().find(|scene| scene.name == name).unwrap();
    (scene.reach)()
}

#[test]
fn reads_home_settings_and_a_ride_page_in_the_light_look() {
    let mut sim = reach("settings light");
    assert_eq!(
        sim.screen().text(),
        "7:00\nGPS\nDISPLAY\nBRIGHTNESS\nAUTO\nDIM AFTER\n30 S\nDAY / NIGHT\nAUTO\nDAY NOW\nTHEME\nLIGHT\nSAVING"
    );
    sim.press(Button::TopLeft);
    assert_eq!(
        sim.screen().text(),
        "7:00\nGPS\nRIDE\nGPS READY\nREADY TO RIDE\nSENSORS\n3/3 LIVE\nHR\nPWR\nSPD/C\nTRAIN\nSETTINGS\nRIDES\nNONE YET"
    );
    sim.start_ride();
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(20));
    assert_eq!(
        sim.screen().text(),
        "7:00\nREC\nGPS\n0:00:19\n2.4%\n28.0\nKM/H\n230\nZ5\nW\n142\nZ1\nBPM\n88\n0.14\nRPM\nKM"
    );
}

struct Painted {
    pixels: Vec<u16>,
    kinds: Vec<Kind>,
}

fn paint(sim: &mut Sim, light: bool) -> Option<Painted> {
    let now = sim.now();
    sim.device.sdk.set_light(light, now);
    sim.device.shell.forget_panel();
    let submitted = sim.device.display.0.borrow().submissions;
    sim.device.tick();
    let frame = sim.device.display.0.borrow();
    if frame.submissions == submitted {
        return None;
    }
    assert_eq!(
        frame.kinds.len(),
        frame.pixels.len(),
        "the frame is recorded"
    );
    Some(Painted {
        pixels: frame.pixels.clone(),
        kinds: frame.kinds.clone(),
    })
}

fn channels(pixel: u16) -> [u8; 3] {
    let (r, g, b) = (pixel >> 11, pixel >> 5 & 0x3f, pixel & 0x1f);
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2].map(|c| c as u8)
}

const GREY_CHROMA: u8 = 8;

const GREY_EDGE_TOLERANCE: [u8; 3] = [17, 8, 17];
const COLOUR_EDGE_TOLERANCE: [u8; 3] = [91, 81, 140];

#[derive(Clone, Copy, Default)]
struct Most {
    by: [u8; 3],
    at: (usize, usize),
}

impl Most {
    fn add(&mut self, delta: [u8; 3], at: (usize, usize)) {
        if delta.iter().max() > self.by.iter().max() {
            self.at = at;
        }
        self.by = core::array::from_fn(|c| self.by[c].max(delta[c]));
    }
}

#[test]
fn every_screen_paints_its_colours_through_the_light_look() {
    record::start();
    let mut corpus = [[0u8; 3]; 2];
    let mut unpainted = Vec::new();
    for scene in SCENES.iter().filter(|scene| !scene.name.contains("light")) {
        let name = scene.name;
        let mut sim = (scene.reach)();
        let Some(shown) = paint(&mut sim, true) else {
            unpainted.push(name);
            continue;
        };
        let lit = sim.screen();
        assert_eq!(lit.look.ink(0), 0xffff, "{name}: read in the light look");
        let dark = paint(&mut sim, false).expect("the dark frame at the same instant");
        let unlit = sim.screen();
        assert_eq!(unlit.look.ink(0), 0, "{name}: read in the dark look");
        let width = unlit.width;
        let code = (sim.device.sdk.page() == Page::Phone)
            .then(|| panel(&dark.pixels, width))
            .flatten();
        let sign = fixed_box(&shown.kinds, width);
        let mut most = [Most::default(); 2];
        for (i, (&was, &now)) in dark.pixels.iter().zip(&shown.pixels).enumerate() {
            let (x, y) = (i % width, i / width);
            if let Some(code) = code.filter(|&code| inside(code, x, y)) {
                if matches!(was, 0 | 0xffff) && !beside_or_in_corner(code, x, y) {
                    assert_eq!(now, was, "{name}: the code at {x},{y}");
                }
                continue;
            }
            match shown.kinds[i] {
                Kind::Mapped => assert_eq!(now, light(was), "{name}: mapped at {x},{y}"),
                Kind::Fixed => assert_eq!(now, was, "{name}: fixed at {x},{y}"),
                Kind::Computed if sign.is_some_and(|sign| inside(sign, x, y)) => {}
                Kind::Computed => {
                    let (now, want) = (channels(now), channels(light(was)));
                    let delta = core::array::from_fn(|c| now[c].abs_diff(want[c]));
                    let was = channels(was);
                    let grey = was.iter().max().unwrap() - was.iter().min().unwrap() <= GREY_CHROMA;
                    let (tolerance, most) = if grey {
                        (GREY_EDGE_TOLERANCE, &mut most[0])
                    } else {
                        (COLOUR_EDGE_TOLERANCE, &mut most[1])
                    };
                    most.add(delta, (x, y));
                    let within = (0..3).all(|c| delta[c] <= tolerance[c]);
                    assert!(within, "{name}: computed at {x},{y}: {now:?} for {want:?}");
                }
                Kind::Unwritten => panic!("{name}: nothing painted {x},{y}"),
            }
        }
        for (screen, corpus) in most.iter().zip(&mut corpus) {
            *corpus = core::array::from_fn(|c| corpus[c].max(screen.by[c]));
        }
        let [grey, colour] = most;
        println!(
            "{name}: grey edges within {:?} (most at {:?}), coloured edges within {:?} (most at {:?})",
            grey.by, grey.at, colour.by, colour.at
        );
        let expected: Vec<_> = unlit
            .frame()
            .read()
            .into_iter()
            .filter(|run| !sign.is_some_and(|sign| inside(sign, run.x, run.y)))
            .map(|run| (run.text, run.x, run.y, light(run.color)))
            .collect();
        let read: Vec<_> = lit
            .frame()
            .read()
            .into_iter()
            .map(|run| (run.text, run.x, run.y, run.color))
            .collect();
        assert_eq!(read, expected, "{name}");
    }
    assert_eq!(unpainted, ["boot"]);
    let [grey, colour] = corpus;
    println!("corpus (r, g, b): grey edges within {grey:?}, coloured edges within {colour:?}");
}

fn panel(pixels: &[u16], width: usize) -> Option<[usize; 4]> {
    let white = pixels
        .iter()
        .enumerate()
        .filter(|&(i, &pixel)| pixel == 0xffff && phone::on_code(i % width, i / width));
    white.fold(None, |found, (i, _)| {
        let (x, y) = (i % width, i / width);
        let [left, top, right, bottom] = found.unwrap_or([x, y, x, y]);
        Some([left.min(x), top.min(y), right.max(x), bottom.max(y)])
    })
}

fn beside_or_in_corner([left, top, right, bottom]: [usize; 4], x: usize, y: usize) -> bool {
    let r = phone::RADIUS;
    let outside = x < left || x > right || y < top || y > bottom;
    outside || (x < left + r || x > right - r) && (y < top + r || y > bottom - r)
}

fn fixed_box(kinds: &[Kind], width: usize) -> Option<[usize; 4]> {
    let fixed = kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| **kind == Kind::Fixed);
    fixed.fold(None, |found, (i, _)| {
        let (x, y) = (i % width, i / width);
        let [left, top, right, bottom] = found.unwrap_or([x, y, x, y]);
        Some([left.min(x), top.min(y), right.max(x), bottom.max(y)])
    })
}

fn inside([left, top, right, bottom]: [usize; 4], x: usize, y: usize) -> bool {
    (left.saturating_sub(1)..=right + 1).contains(&x)
        && (top.saturating_sub(1)..=bottom + 1).contains(&y)
}
