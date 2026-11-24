use device_api::input::Role;
use firmware_shell::rendering::record::{self, Kind};
use shell_simulator::bench::SCENES;
use shell_simulator::sim::{Sim, seconds};
use vana::screens::chrome::{
    ACCENT, AMBER, BLACK, CARD, GREEN, HEART, MUTED, POWER, PRESSED, RAISED, RED, SECONDARY, SPEED,
    TERTIARY, WHITE, ZONES,
};
use vana::screens::look::light;
use vana::screens::sign::{ARRIVED_FILL, OFF_FILL, TURN_FILL};

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
    sim.press(Role::Back);
    assert_eq!(
        sim.screen().text(),
        "7:00\nGPS\nRIDE\nGPS READY\nREADY TO RIDE\nSENSORS\n3/3 LIVE\nHR\nPWR\nSPD/C\nTRAIN\nSETTINGS"
    );
    sim.start_ride();
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(20));
    assert_eq!(
        sim.screen().text(),
        "0:00:20\nPWR 3S W\nZ5\nHEART BPM\nZ1\n230\n142\nSPD KM/H\nCAD RPM\n28.0\n88\nGRADE\nCLIMB M\n2.4\n3\n%\nDIST KM\nNP W\n0.2\n--"
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

const GREY_ROUNDING_STEPS: u16 = 1;

const PALETTE: [u16; 16] = [
    BLACK, WHITE, SECONDARY, MUTED, TERTIARY, CARD, RAISED, PRESSED, ACCENT, GREEN, AMBER, RED,
    SPEED, POWER, HEART, ZONES[6],
];

const SIGN_FILLS: [u16; 3] = [TURN_FILL, OFF_FILL, ARRIVED_FILL];

const SIGN_RADIUS: usize = 12;

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
        // Kit pages draw in their theme's own colours, not the dark
        // colours mapped through the look.
        if matches!(
            sim.ride_page_name().as_deref(),
            Some("ride/main" | "ride/climb" | "ride/workout")
        ) {
            continue;
        }
        let Some(shown) = paint(&mut sim, true) else {
            unpainted.push(name);
            continue;
        };
        let lit = sim.screen();
        assert_eq!(lit.look.ink(0).0, 0xffff, "{name}: read in the light look");
        let dark = paint(&mut sim, false).expect("the dark frame at the same instant");
        let unlit = sim.screen();
        assert_eq!(unlit.look.ink(0).0, 0, "{name}: read in the dark look");
        let width = unlit.width;
        let sign = sign_box(&shown, width, name);
        let mut most = [Most::default(); 2];
        for (i, (&was, &now)) in dark.pixels.iter().zip(&shown.pixels).enumerate() {
            let (x, y) = (i % width, i / width);
            match shown.kinds[i] {
                Kind::Mapped => assert_eq!(now, light(was), "{name}: mapped at {x},{y}"),
                Kind::Fixed => assert_eq!(now, was, "{name}: fixed at {x},{y}"),
                Kind::Computed if sign.is_some_and(|sign| inside(sign, x, y)) => {}
                Kind::Computed if solid(&dark.pixels, width, i) => {
                    let want = light(was);
                    let rounding = if grey(was) { GREY_ROUNDING_STEPS } else { 0 };
                    assert!(
                        max_channel_steps(now, want) <= rounding,
                        "{name}: solid computed at {x},{y}: {now:#06x} for {want:#06x}"
                    );
                }
                Kind::Computed => {
                    let (now, want) = (channels(now), channels(light(was)));
                    let delta = core::array::from_fn(|c| now[c].abs_diff(want[c]));
                    let (tolerance, most) = if grey(was) {
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
            .filter(|run| !sign.is_some_and(|sign| inside(sign, run.x, run.y)))
            .map(|run| (run.text, run.x, run.y, run.color))
            .collect();
        assert_eq!(read, expected, "{name}");
    }
    assert_eq!(unpainted, ["boot"]);
    let [grey, colour] = corpus;
    println!("corpus (r, g, b): grey edges within {grey:?}, coloured edges within {colour:?}");
}

fn solid(pixels: &[u16], width: usize, i: usize) -> bool {
    let x = i % width;
    PALETTE.contains(&pixels[i])
        && x > 0
        && x + 1 < width
        && pixels[i - 1] == pixels[i]
        && pixels[i + 1] == pixels[i]
}

fn grey(pixel: u16) -> bool {
    let channels = channels(pixel);
    channels.iter().max().unwrap() - channels.iter().min().unwrap() <= GREY_CHROMA
}

fn max_channel_steps(a: u16, b: u16) -> u16 {
    [(11, 0x1f), (5, 0x3f), (0, 0x1f)]
        .into_iter()
        .map(|(shift, mask)| (a >> shift & mask).abs_diff(b >> shift & mask))
        .max()
        .unwrap()
}

fn bounds(width: usize, at: impl Iterator<Item = usize>) -> Option<[usize; 4]> {
    at.fold(None, |found, i| {
        let (x, y) = (i % width, i / width);
        let [left, top, right, bottom] = found.unwrap_or([x, y, x, y]);
        Some([left.min(x), top.min(y), right.max(x), bottom.max(y)])
    })
}

fn sign_box(shown: &Painted, width: usize, name: &str) -> Option<[usize; 4]> {
    let fixed = || (0..shown.kinds.len()).filter(|&i| shown.kinds[i] == Kind::Fixed);
    let sign = bounds(
        width,
        fixed().filter(|&i| SIGN_FILLS.contains(&shown.pixels[i])),
    );
    for i in fixed() {
        let (x, y) = (i % width, i / width);
        let on_sign = sign.is_some_and(|sign| inside(sign, x, y));
        assert!(on_sign, "{name}: fixed at {x},{y} outside the sign");
    }
    if let Some([left, top, right, bottom]) = sign {
        let fill = |x: usize, y: usize| {
            let i = y * width + x;
            shown.kinds[i] == Kind::Fixed && SIGN_FILLS.contains(&shown.pixels[i])
        };
        let r = SIGN_RADIUS;
        let (across, down) = (left + r..=right - r, top + r..=bottom - r);
        let edges = right - left > 2 * r
            && bottom - top > 2 * r
            && across.clone().all(|x| fill(x, top) && fill(x, bottom))
            && down.clone().all(|y| fill(left, y) && fill(right, y));
        assert!(
            edges,
            "{name}: the sign's fill stops short of its box {sign:?}"
        );
    }
    sign
}

fn inside([left, top, right, bottom]: [usize; 4], x: usize, y: usize) -> bool {
    (left.saturating_sub(1)..=right + 1).contains(&x)
        && (top.saturating_sub(1)..=bottom + 1).contains(&y)
}
