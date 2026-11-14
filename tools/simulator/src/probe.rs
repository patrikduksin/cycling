//! The redraw probe (#369): run a page for simulated seconds under a named
//! sensor feed and compare every submitted frame with the one before.
//!
//! The probe says how much of the screen a redraw engine would have had to
//! repaint: which of the panel's 16-row strips changed, how many pixels, and
//! how many frames changed nothing at all. `redraw-probe` prints the table.
use crate::bench::SCENES;
use crate::sim::Sim;
use crate::world::{Side, Vehicle};

/// Rows the device sends in one strip.
pub const STRIP_ROWS: usize = 16;

/// What one submitted frame changed from the frame before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub strips: usize,
    pub pixels: usize,
}

/// Compares each shown frame with the last, as `Frame::shown` hands them over.
pub struct Probe {
    width: usize,
    previous: Vec<u16>,
    pub changes: Vec<Change>,
}

impl Probe {
    /// Starts from the frame on the panel now, which is not counted.
    pub fn new(width: usize, pixels: &[u16]) -> Self {
        Self {
            width,
            previous: pixels.to_vec(),
            changes: Vec::new(),
        }
    }

    pub fn observe(&mut self, pixels: &[u16]) {
        let strip = self.width * STRIP_ROWS;
        let strips = self
            .previous
            .chunks(strip)
            .zip(pixels.chunks(strip))
            .filter(|(a, b)| a != b)
            .count();
        let pixel_count = self
            .previous
            .iter()
            .zip(pixels)
            .filter(|(a, b)| a != b)
            .count();
        self.changes.push(Change {
            strips,
            pixels: pixel_count,
        });
        self.previous.copy_from_slice(pixels);
    }
}

/// What the sensors do while the page is watched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    /// The scene's readings hold; with `cars`, a vehicle appears about every 7 s.
    Steady { cars: bool },
    /// Power within 35 W, and heart rate, speed and cadence, change every 250 ms.
    Noisy { cars: bool },
    /// The readings hold and the battery charges at `percent`, as on USB.
    Charging { percent: u8 },
}

pub struct Case {
    pub name: &'static str,
    /// A scene of `bench::SCENES`.
    pub scene: &'static str,
    pub feed: Feed,
    /// The rider chose the display mode SAVER first. Only "ride main + radar"
    /// can be reached that way.
    pub saver: bool,
}

pub static CASES: [Case; 12] = [
    Case {
        name: "ride main, noisy, radar",
        scene: "ride main + radar",
        feed: Feed::Noisy { cars: true },
        saver: false,
    },
    Case {
        name: "ride main, noisy, radar, saver",
        scene: "ride main + radar",
        feed: Feed::Noisy { cars: true },
        saver: true,
    },
    Case {
        name: "ride main, steady, radar",
        scene: "ride main + radar",
        feed: Feed::Steady { cars: true },
        saver: false,
    },
    Case {
        name: "climb",
        scene: "climb",
        feed: Feed::Steady { cars: false },
        saver: false,
    },
    Case {
        name: "workout",
        scene: "workout",
        feed: Feed::Steady { cars: false },
        saver: false,
    },
    Case {
        name: "stats",
        scene: "ride stats",
        feed: Feed::Steady { cars: false },
        saver: false,
    },
    Case {
        name: "map",
        scene: "map",
        feed: Feed::Steady { cars: false },
        saver: false,
    },
    Case {
        name: "settings (still)",
        scene: "settings",
        feed: Feed::Steady { cars: false },
        saver: false,
    },
    Case {
        name: "home",
        scene: "home",
        feed: Feed::Steady { cars: false },
        saver: false,
    },
    Case {
        name: "home, charging at 50%",
        scene: "home",
        feed: Feed::Charging { percent: 50 },
        saver: false,
    },
    Case {
        name: "home, charging at 99%",
        scene: "home",
        feed: Feed::Charging { percent: 99 },
        saver: false,
    },
    Case {
        name: "home, charging at 100%",
        scene: "home",
        feed: Feed::Charging { percent: 100 },
        saver: false,
    },
];

const TICK_MS: u64 = 250;
const CAR_EVERY_MS: u64 = 7_000;

/// What the probe saw over a run.
#[derive(Clone, Debug)]
pub struct Result {
    pub seconds: u64,
    pub changes: Vec<Change>,
    pub screen_pixels: usize,
    pub strips: usize,
}

impl Result {
    pub fn fps(&self) -> f64 {
        self.changes.len() as f64 / self.seconds as f64
    }

    pub fn identical(&self) -> usize {
        self.changes.iter().filter(|c| c.pixels == 0).count()
    }

    /// Median and 90th percentile of changed strips over every frame.
    pub fn strips(&self) -> (usize, usize) {
        percentiles(self.changes.iter().map(|c| c.strips).collect())
    }

    /// Median and 90th percentile of changed pixels, as a share of the screen.
    pub fn pixels(&self) -> (f64, f64) {
        let (median, p90) = percentiles(self.changes.iter().map(|c| c.pixels).collect());
        let share = |n: usize| n as f64 / self.screen_pixels as f64;
        (share(median), share(p90))
    }
}

fn percentiles(mut values: Vec<usize>) -> (usize, usize) {
    values.sort_unstable();
    let at = |per_cent: usize| {
        (values.len() * per_cent)
            .div_ceil(100)
            .checked_sub(1)
            .map_or(0, |rank| values[rank])
    };
    (at(50), at(90))
}

/// A small deterministic noise source, so a run repeats exactly.
struct Noise(u64);

impl Noise {
    /// A value in -1.0..=1.0.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 2001) as f64 / 1000.0 - 1.0
    }
}

/// Reach the case's scene, then watch it for `seconds` of simulated time.
pub fn run(case: &Case, seconds: u64) -> Result {
    let scene = SCENES
        .iter()
        .find(|scene| scene.name == case.scene)
        .unwrap_or_else(|| panic!("no scene {}", case.scene));
    let mut sim = if case.saver {
        assert_eq!(case.scene, "ride main + radar", "{}", case.name);
        crate::bench::ride_main_saver()
    } else {
        (scene.reach)()
    };
    let display = sim.device.display.clone();
    {
        let mut frame = display.0.borrow_mut();
        frame.check = false;
        frame.probe = Some(Probe::new(frame.geometry.width, &frame.pixels));
    }
    feed(&mut sim, case.feed, seconds);
    let mut frame = display.0.borrow_mut();
    let (width, height) = (frame.geometry.width, frame.geometry.height);
    Result {
        seconds,
        changes: frame.probe.take().map(|p| p.changes).unwrap_or_default(),
        screen_pixels: width * height,
        strips: height.div_ceil(STRIP_ROWS),
    }
}

fn feed(sim: &mut Sim, feed: Feed, seconds: u64) {
    let world = sim.world();
    let base = (
        world.speed_kmh,
        world.power_w,
        world.heart_bpm,
        world.cadence_rpm,
    );
    if let Feed::Charging { percent } = feed {
        sim.set_battery(percent, true);
    }
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
    let mut cars = 0u64;
    for tick in 0..seconds * 1_000 / TICK_MS {
        let now = tick * TICK_MS;
        if let Feed::Noisy { .. } = feed {
            sim.ride_at(
                base.0 + noise.next(),
                base.1 + 35.0 * noise.next(),
                base.2 + 2.0 * noise.next(),
                base.3 + 3.0 * noise.next(),
            );
        }
        if let Feed::Steady { cars: true } | Feed::Noisy { cars: true } = feed
            && now / CAR_EVERY_MS >= cars
        {
            cars += 1;
            sim.vehicle(Vehicle {
                range_m: 100.0,
                closing_kmh: 25.0,
                fast: false,
                side: [Side::Behind, Side::Right, Side::Left][cars as usize % 3],
            });
        }
        sim.advance(TICK_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_counts_the_strips_and_pixels_that_differ_from_the_one_before() {
        let (width, height) = (4, 48);
        let blank = vec![0u16; width * height];
        let mut probe = Probe::new(width, &blank);
        let mut frame = blank.clone();
        frame[0] = 1;
        frame[width * 17 + 2] = 1;
        probe.observe(&frame);
        probe.observe(&frame);
        assert_eq!(
            probe.changes,
            [
                Change {
                    strips: 2,
                    pixels: 2
                },
                Change {
                    strips: 0,
                    pixels: 0
                }
            ]
        );
    }

    #[test]
    fn percentiles_pick_the_median_and_the_ninetieth() {
        assert_eq!(percentiles((1..=10).collect()), (5, 9));
        assert_eq!(percentiles([0, 0, 3, 3].into()), (0, 3));
        assert_eq!(percentiles(Vec::new()), (0, 0));
    }
}
