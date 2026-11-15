use crate::bench::SCENES;
use crate::sim::Sim;
use crate::world::{Side, Vehicle};
use firmware_shell::rendering::work::Work;

pub const STRIP_ROWS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub strips: usize,
    pub pixels: usize,
    pub rows: usize,
    pub work: u64,
}

pub fn pixel_work(work: &Work) -> u64 {
    work.span_pixels + work.evaluations + work.glyph_columns
}

pub struct Probe {
    width: usize,
    previous: Vec<u16>,
    pub changes: Vec<Change>,
}

impl Probe {
    pub fn new(width: usize, pixels: &[u16]) -> Self {
        Self {
            width,
            previous: pixels.to_vec(),
            changes: Vec::new(),
        }
    }

    pub fn observe(&mut self, pixels: &[u16], rows: usize, work: &Work) {
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
            rows,
            work: pixel_work(work),
        });
        self.previous.copy_from_slice(pixels);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    Steady { cars: bool },
    Noisy { cars: bool },
    Charging { percent: u8 },
}

pub struct Case {
    pub name: &'static str,
    pub scene: &'static str,
    pub feed: Feed,
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

    pub fn strips(&self) -> (usize, usize) {
        percentiles(self.changes.iter().map(|c| c.strips).collect())
    }

    pub fn rows(&self) -> (usize, usize) {
        percentiles(self.changes.iter().map(|c| c.rows).collect())
    }

    pub fn rows_per_second(&self) -> f64 {
        self.changes.iter().map(|c| c.rows).sum::<usize>() as f64 / self.seconds as f64
    }

    pub fn work_per_second(&self) -> f64 {
        self.changes.iter().map(|c| c.work).sum::<u64>() as f64 / self.seconds as f64
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

struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 2001) as f64 / 1000.0 - 1.0
    }
}

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
    fn a_frame_counts_what_differs_from_the_one_before_and_what_painting_it_took() {
        let (width, height) = (4, 48);
        let blank = vec![0u16; width * height];
        let mut probe = Probe::new(width, &blank);
        let mut frame = blank.clone();
        frame[0] = 1;
        frame[width * 17 + 2] = 1;
        let work = Work {
            span_pixels: 10,
            evaluations: 2,
            glyph_columns: 3,
            ..Work::default()
        };
        probe.observe(&frame, height, &work);
        probe.observe(&frame, 4, &Work::default());
        assert_eq!(
            probe.changes,
            [
                Change {
                    strips: 2,
                    pixels: 2,
                    rows: height,
                    work: 15,
                },
                Change {
                    strips: 0,
                    pixels: 0,
                    rows: 4,
                    work: 0,
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
