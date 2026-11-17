use super::*;
use firmware_services::testing::Rng;
use firmware_services::trace::Medium;
use firmware_services::trace::determined::{Determined, Span};

const SIZE: usize = 6 * 1024;

struct World {
    device: Vec<u8>,
    known: Vec<bool>,
    determined: Determined,
    twin: TwinMedium,
    rng: Rng,
}

impl World {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let device = (0..SIZE).map(|_| rng.byte()).collect();
        Self {
            device,
            known: vec![false; SIZE],
            determined: Determined::new(),
            twin: TwinMedium::new(),
            rng,
        }
    }

    fn span(at: usize, len: usize) -> Span {
        Span {
            medium: Medium::Data,
            start: at as u32,
            count: len as u32,
        }
    }

    fn read(&mut self, at: usize, len: usize) {
        let span = Self::span(at, len);
        if !self.determined.covers(span) {
            self.twin.install(at as u64, &self.device[at..at + len]);
            self.known[at..at + len].fill(true);
            self.determined.insert(span);
        }
        let mut out = vec![0; len];
        assert_eq!(
            self.twin.read(at as u64, &mut out),
            Ok(()),
            "a read the device recorded only as a digest is unknown to the twin"
        );
        assert_eq!(out, self.device[at..at + len], "the twin read other bytes");
    }

    fn mutate(&mut self, at: usize, change: Change<'_>, outcome: Outcome) {
        let len = change.len() as usize;
        let span = Self::span(at, len);
        let range = at..at + len;
        if outcome == Outcome::Failed {
            let landed = self.rng.below(len as u64 + 1) as usize;
            for byte in &mut self.device[at..at + landed] {
                *byte = self.rng.byte();
            }
            self.known[range].fill(false);
            self.determined.remove(span);
        } else {
            match change {
                Change::Write(bytes) => {
                    self.device[range.clone()].copy_from_slice(bytes);
                    self.known[range].fill(true);
                    self.determined.insert(span);
                }
                Change::Program(bytes) => {
                    for (byte, with) in self.device[range].iter_mut().zip(bytes) {
                        *byte &= with;
                    }
                    if !self.determined.covers(span) {
                        self.determined.remove(span);
                    }
                }
                Change::Erase { .. } => {
                    self.device[range.clone()].fill(0xFF);
                    self.known[range].fill(true);
                    self.determined.insert(span);
                }
            }
        }
        self.twin.mutate(at as u64, change, outcome);
    }

    fn host_write(&mut self) {
        let at = self.rng.below(SIZE as u64) as usize;
        let len = self.rng.below((SIZE - at) as u64 + 1) as usize;
        for byte in &mut self.device[at..at + len] {
            *byte = self.rng.byte();
        }
        self.known.fill(false);
        self.determined.forget(Medium::Data);
        self.twin.forget();
    }

    fn assert_matches_the_device(&mut self, seed: u64, step: u32) {
        let mut image = vec![None; SIZE];
        for (number, page) in &self.twin.pages {
            for i in 0..PAGE {
                let at = *number as usize * PAGE + i;
                if let (Some(byte), Some(slot)) = (page.get(i), image.get_mut(at)) {
                    *slot = Some(byte);
                }
            }
        }
        let expected: Vec<_> = self
            .known
            .iter()
            .zip(&self.device)
            .map(|(known, byte)| known.then_some(*byte))
            .collect();
        if let Some(at) = (0..SIZE).find(|&at| image[at] != expected[at]) {
            panic!(
                "seed {seed} step {step}: the twin holds {:?} at byte {at}, where the device holds {:?}",
                image[at], expected[at]
            );
        }
        let (at, len) = any_range(&mut self.rng, (0, 1));
        let mut out = vec![0; len];
        let first_unknown = (at..at + len).find(|&at| !self.known[at]);
        match first_unknown {
            Some(unknown) => assert_eq!(
                self.twin.read(at as u64, &mut out),
                Err(Unknown { at: unknown as u64 })
            ),
            None => {
                assert_eq!(self.twin.read(at as u64, &mut out), Ok(()));
                assert_eq!(out, self.device[at..at + len]);
            }
        }
    }
}

fn any_range(rng: &mut Rng, last: (usize, usize)) -> (usize, usize) {
    let (at, len) = if rng.one_in(3) {
        let at = last.0 + rng.below(last.1 as u64 + 1) as usize;
        (at.min(SIZE - 1), rng.range(1, 300) as usize)
    } else {
        (rng.below(SIZE as u64) as usize, rng.range(1, 300) as usize)
    };
    (at, len.min(SIZE - at))
}

#[test]
fn the_twin_knows_exactly_the_bytes_the_trace_gave_and_they_are_the_devices() {
    for seed in 0..120 {
        let mut world = World::new(seed);
        let mut last = (0, 1);
        for step in 0..400 {
            let (at, len) = any_range(&mut world.rng, last);
            last = (at, len);
            let outcome = if world.rng.one_in(8) {
                Outcome::Failed
            } else {
                Outcome::Done
            };
            let bytes: Vec<u8> = (0..len).map(|_| world.rng.byte()).collect();
            match world.rng.below(16) {
                0..=5 => world.read(at, len),
                6..=8 => world.mutate(at, Change::Write(&bytes), outcome),
                9..=11 => world.mutate(at, Change::Program(&bytes), outcome),
                12..=13 => world.mutate(at, Change::Erase { len: len as u64 }, outcome),
                14 if world.rng.one_in(6) => world.host_write(),
                _ => world.determined.next_pass(),
            }
            world.assert_matches_the_device(seed, step);
        }
    }
}

#[test]
fn a_program_onto_unknown_bytes_leaves_them_unknown_and_ands_onto_known_ones() {
    let mut twin = TwinMedium::new();
    twin.install(0, &[0xF0, 0xFF]);
    twin.mutate(0, Change::Program(&[0x3C, 0x0F, 0x00]), Outcome::Done);
    let mut out = [0; 2];
    assert_eq!(twin.read(0, &mut out), Ok(()));
    assert_eq!(out, [0x30, 0x0F]);
    assert_eq!(twin.read(0, &mut [0; 3]), Err(Unknown { at: 2 }));
}

#[test]
fn a_failed_erase_forgets_every_byte_it_aimed_at() {
    let mut twin = TwinMedium::new();
    twin.install(0, &[1; 1024]);
    twin.mutate(100, Change::Erase { len: 600 }, Outcome::Failed);
    assert_eq!(twin.read(0, &mut [0; 100]), Ok(()));
    assert_eq!(twin.read(0, &mut [0; 101]), Err(Unknown { at: 100 }));
    assert_eq!(twin.read(700, &mut [0; 324]), Ok(()));
    assert_eq!(twin.read(699, &mut [0; 1]), Err(Unknown { at: 699 }));
}

#[test]
fn a_host_write_forgets_the_whole_medium() {
    let mut twin = TwinMedium::new();
    twin.mutate(4096, Change::Write(&[7; 8]), Outcome::Done);
    twin.forget();
    assert_eq!(twin.read(4096, &mut [0; 1]), Err(Unknown { at: 4096 }));
}
