//! After a restart: the breadcrumb and the start of a ride the restart cut
//! short, read back from the journal for the ride that carries on from it.
//!
//! A restart closes the open ride as recovered, and the rider starts the next
//! ride to ride on. That ride reads the recovered ride's fixes back into the
//! breadcrumb (`Breadcrumb::replay`), then its own fixes so far, a budget of slots
//! at a time, and live fixes follow once it has read every slot written. Its
//! own first fix decides whether it carries on: within `CONTINUE_MS` of the
//! recovered ride's last fix it does; later, or without the time of either,
//! the breadcrumb starts again from it, as a new ride's does. Once every slot
//! is read, a live fix decides as well (`settle`), since a paused ride or a
//! failing medium may never journal one. Nothing new is stored: the journal
//! already holds every fix.
use super::Breadcrumb;
use crate::ride::log::{Entry, Kind};
use crate::ride::replay::Walk;

/// The next ride carries on the recovered one if its first fix is this soon
/// after the recovered ride's last.
pub const CONTINUE_MS: u64 = 30 * 60_000;
/// The journal holds a sample a second while recording.
const SAMPLE_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Before the ride's own first fix: when the recovered ride's latest fix
    /// was, if known.
    Pending { last_utc: Option<u64> },
    /// The ride's own first fix came: the breadcrumb is settled.
    Decided,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rebuild {
    recovered: u32,
    walk: Walk,
    stage: Stage,
    /// Time through the replayed samples, for the heading.
    clock_ms: u64,
}

impl Rebuild {
    /// Read ride `recovered` and the ride after it back from a journal whose
    /// slots below `upper` are written.
    pub fn new(recovered: u32, upper: usize) -> Self {
        Self {
            recovered,
            walk: Walk::new(recovered, upper),
            stage: Stage::Pending { last_utc: None },
            clock_ms: 0,
        }
    }

    /// Read up to `budget` more slots below `upper` through `read` into
    /// `breadcrumb`; true once every slot is read and the breadcrumb settled, when
    /// live fixes take over (`Breadcrumb::end_replay`).
    pub fn step(
        &mut self,
        breadcrumb: &mut Breadcrumb,
        upper: usize,
        budget: usize,
        read: impl FnMut(usize) -> Option<Entry>,
    ) -> bool {
        let mut walk = self.walk;
        walk.step(upper, budget, read, |entry| {
            self.visit(breadcrumb, entry);
            true
        });
        self.walk = walk;
        self.stage == Stage::Decided && self.walk.caught_up(upper)
    }

    /// Every slot below `upper` is read.
    pub fn caught_up(&self, upper: usize) -> bool {
        self.walk.caught_up(upper)
    }

    /// Settle the breadcrumb on a live fix at `utc_ms` instead of the ride's
    /// own first journaled one, which a paused ride or a failing medium may
    /// never write.
    pub fn settle(&mut self, breadcrumb: &mut Breadcrumb, utc_ms: Option<u64>) {
        if let Stage::Pending { last_utc } = self.stage {
            self.decide(breadcrumb, last_utc, utc_ms);
        }
    }

    fn decide(
        &mut self,
        breadcrumb: &mut Breadcrumb,
        last_utc: Option<u64>,
        first_utc: Option<u64>,
    ) {
        let carries_on = last_utc
            .zip(first_utc)
            .is_some_and(|(last, first)| last.abs_diff(first) <= CONTINUE_MS);
        if !carries_on {
            breadcrumb.reset();
        }
        self.stage = Stage::Decided;
    }

    fn visit(&mut self, breadcrumb: &mut Breadcrumb, entry: &Entry) {
        if entry.kind != Kind::Samples {
            return;
        }
        let own = entry.ride_id != self.recovered;
        for sample in &entry.samples[..usize::from(entry.count)] {
            self.clock_ms = self.clock_ms.saturating_add(SAMPLE_MS);
            let Some(location) = sample.location_e7 else {
                continue;
            };
            match self.stage {
                Stage::Pending { last_utc } if !own => {
                    self.stage = Stage::Pending {
                        last_utc: sample.utc_ms.or(last_utc),
                    };
                }
                Stage::Pending { last_utc } => self.decide(breadcrumb, last_utc, sample.utc_ms),
                Stage::Decided if !own => continue,
                Stage::Decided => {}
            }
            breadcrumb.replay(location, self.clock_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::breadcrumb::Crumb;
    use crate::ride::breadcrumb::tests::{at, taken};
    use crate::ride::course;
    use crate::ride::course::follow::{Follower, Input};
    use crate::ride::course::nav::Event;
    use crate::ride::log::Sample;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    type Fixes = Vec<(i32, i32)>;

    const UTC: u64 = 1_790_000_000_000;

    /// A 3 km square loop, a fix every 8 m, ridden `laps` times.
    fn square(laps: usize) -> Vec<(i32, i32)> {
        let mut fixes = Vec::new();
        for _ in 0..laps {
            for i in 0..375 {
                let d = i as f32 * 8.0;
                let (east, north) = match d {
                    d if d < 750.0 => (d, 0.0),
                    d if d < 1500.0 => (750.0, d - 750.0),
                    d if d < 2250.0 => (2250.0 - d, 750.0),
                    d => (0.0, 3000.0 - d),
                };
                fixes.push(at(east, north));
            }
        }
        fixes
    }

    /// Ride `id`'s journal: START, a sample a second from `utc` at the fixes,
    /// and its closing record.
    fn ride(id: u32, fixes: &[Option<(i32, i32)>], utc: Option<u64>, end: Kind) -> Vec<Entry> {
        let mut slots = vec![Entry::event(Kind::Start, id, 0, 0)];
        let samples: Vec<Sample> = fixes
            .iter()
            .enumerate()
            .map(|(i, &location_e7)| Sample {
                active_ms: i as u64 * 1_000,
                utc_ms: utc.map(|utc| utc + i as u64 * 1_000),
                location_e7,
                ..Sample::default()
            })
            .collect();
        for (i, batch) in samples.chunks(4).enumerate() {
            slots.push(Entry::batch(id, i as u32 + 1, batch).unwrap());
        }
        let last = samples.len() as u64 * 1_000;
        slots.push(Entry::event(end, id, samples.len() as u32, last));
        slots
    }

    fn located(fixes: &[(i32, i32)]) -> Vec<Option<(i32, i32)>> {
        fixes.iter().copied().map(Some).collect()
    }

    /// Step `rebuild` with `budget` over `slots` until done; returns the
    /// steps taken and the most slots one step read.
    fn rebuild(
        breadcrumb: &mut Breadcrumb,
        slots: &[Entry],
        recovered: u32,
        budget: usize,
    ) -> (usize, usize) {
        let mut rebuild = Rebuild::new(recovered, slots.len());
        let (mut steps, mut most) = (0, 0);
        loop {
            steps += 1;
            assert!(steps < 100_000, "the rebuild never ends");
            let mut reads = 0;
            let done = rebuild.step(breadcrumb, slots.len(), budget, |i| {
                reads += 1;
                slots.get(i).copied()
            });
            most = most.max(reads);
            if done {
                breadcrumb.end_replay();
                return (steps, most);
            }
        }
    }

    fn live(breadcrumb: &mut Breadcrumb, fixes: &[(i32, i32)], now: &mut u64) {
        for &fix in fixes {
            *now += 1_000;
            breadcrumb.observe(taken(fix, *now), *now);
        }
    }

    /// A ride of two laps cut short half way round the third, then the next
    /// ride's first 40 s, on its way round.
    fn restarted() -> (Fixes, Fixes, Vec<Entry>) {
        let fixes = square(3);
        let (before, after) = (fixes[..930].to_vec(), fixes[935..975].to_vec());
        let mut slots = ride(1, &located(&before), Some(UTC), Kind::Recovered);
        let resumed = UTC + 960_000;
        slots.extend(ride(2, &located(&after), Some(resumed), Kind::Finish));
        (before, after, slots)
    }

    #[test]
    fn a_restarted_ride_reads_back_the_breadcrumb_it_had() {
        let (before, after, slots) = restarted();
        let mut crumbs = vec![Crumb::default(); 512];
        let mut uncut = Breadcrumb::new(&mut crumbs);
        let mut now = 0;
        live(&mut uncut, &before, &mut now);
        live(&mut uncut, &after, &mut now);

        let mut crumbs = vec![Crumb::default(); 512];
        let mut read = Breadcrumb::new(&mut crumbs);
        let (steps, most) = rebuild(&mut read, &slots, 1, 8);
        assert!(most <= 8, "{most} slots in one step");
        assert!(
            steps > slots.len() / 8,
            "{steps} steps for {} slots",
            slots.len()
        );
        assert_eq!(read.crumbs(), uncut.crumbs());
        assert!(read.guide().is_none(), "no guide until a live fix");

        let next = square(3)[975..990].to_vec();
        live(&mut uncut, &next, &mut now);
        live(&mut read, &next, &mut 10_000_000);
        let (read_m, uncut_m) = (read.way_back().unwrap().1, uncut.way_back().unwrap().1);
        assert!(
            read_m.abs_diff(uncut_m) <= 2,
            "{read_m} m against {uncut_m} m"
        );
    }

    /// Five hours on winding roads, a fix a second: 4,500 slots read a few
    /// at a time give the breadcrumb the ride had, thinned as it was in a
    /// breadcrumb a quarter the device's.
    #[test]
    fn a_five_hour_ride_reads_back_a_few_slots_a_step() {
        let road = course::tests::road(150.0, 8.0, 7);
        let fixes: Vec<(i32, i32)> = road
            .iter()
            .map(|p| (p.lat_e7, p.lon_e7))
            .take(18_000)
            .collect();
        let (before, after) = fixes.split_at(17_960);
        let mut slots = ride(1, &located(before), Some(UTC), Kind::Recovered);
        slots.extend(ride(
            2,
            &located(after),
            Some(UTC + 18_000_000),
            Kind::Finish,
        ));
        assert!(slots.len() > 4_500, "{} slots", slots.len());

        let mut crumbs = vec![Crumb::default(); 1024];
        let mut uncut = Breadcrumb::new(&mut crumbs);
        live(&mut uncut, &fixes, &mut 0);
        assert!(
            uncut.tolerance > super::super::TOLERANCE_M,
            "the ride thinned"
        );

        let mut crumbs = vec![Crumb::default(); 1024];
        let mut read = Breadcrumb::new(&mut crumbs);
        let (steps, most) = rebuild(&mut read, &slots, 1, 32);
        assert!(most <= 32, "{most} slots in one step");
        assert!(steps >= slots.len() / 32, "{steps} steps");
        assert_eq!(read.crumbs(), uncut.crumbs());
    }

    #[test]
    fn the_start_comes_back_and_the_next_pass_takes_a_lap() {
        let (_, _, slots) = restarted();
        let mut crumbs = vec![Crumb::default(); 512];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        rebuild(&mut breadcrumb, &slots, 1, 16);
        assert!(!breadcrumb.take_pass(), "two laps read back take none");
        let mut now = 0;
        // On round the rest of the third lap, past the start.
        let rest = square(4)[975..1150].to_vec();
        live(&mut breadcrumb, &rest, &mut now);
        assert!(
            breadcrumb.take_pass(),
            "the pass of the ride's start is a lap"
        );
        assert!(!breadcrumb.take_pass());
    }

    #[test]
    fn a_ride_long_after_the_recovered_one_starts_its_own_breadcrumb() {
        let fixes = square(1);
        let (old, new) = (&fixes[..300], &fixes[100..200]);
        for (utc, gap) in [(Some(UTC), CONTINUE_MS + 1_000), (None, 0)] {
            let mut slots = ride(4, &located(old), utc, Kind::Recovered);
            let resumed = utc.map(|utc| utc + 300_000 + gap);
            slots.extend(ride(5, &located(new), resumed, Kind::Finish));
            let mut crumbs = vec![Crumb::default(); 256];
            let mut breadcrumb = Breadcrumb::new(&mut crumbs);
            rebuild(&mut breadcrumb, &slots, 4, 32);
            let first = breadcrumb.crumbs()[0];
            let start = breadcrumb.frame().unwrap().place(new[0].0, new[0].1);
            assert_eq!((first.east, first.north), (0.0, 0.0), "{utc:?}");
            assert_eq!(start, (0.0, 0.0), "the new ride's first fix is the origin");
            assert!(breadcrumb.crumbs().last().unwrap().along < 820.0, "{utc:?}");
        }
    }

    #[test]
    fn the_rebuild_waits_for_the_rides_own_first_fix() {
        let fixes = square(1);
        let mut slots = ride(1, &located(&fixes[..200]), Some(UTC), Kind::Recovered);
        // The ride after it has no fix yet: the receiver is still searching.
        slots.extend(ride(2, &[None; 12], Some(UTC + 240_000), Kind::Samples));
        slots.pop();
        let mut crumbs = vec![Crumb::default(); 256];
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut rebuild = Rebuild::new(1, slots.len());
        for _ in 0..20 {
            let done = rebuild.step(&mut breadcrumb, slots.len(), 16, |i| slots.get(i).copied());
            assert!(!done, "settled before the ride's own first fix");
        }
        let kept = breadcrumb.crumbs().len();
        assert!(kept >= 2);
        // Its first fix is journaled: the track carries on from it.
        let first = Sample {
            active_ms: 12_000,
            utc_ms: Some(UTC + 252_000),
            location_e7: Some(fixes[205]),
            ..Sample::default()
        };
        slots.push(Entry::batch(2, 4, &[first]).unwrap());
        assert!(rebuild.step(&mut breadcrumb, slots.len(), 16, |i| slots.get(i).copied()));
        assert!(breadcrumb.crumbs().len() >= kept);
        assert_eq!(
            breadcrumb.crumbs()[0],
            Crumb::default(),
            "the recovered ride's start"
        );
    }

    /// A ride paused from its start journals no fix: once every slot is
    /// read, a live fix settles the track by the same rule.
    #[test]
    fn a_live_fix_settles_a_ride_that_journals_none() {
        let fixes = square(1);
        let slots = ride(1, &located(&fixes[..200]), Some(UTC), Kind::Recovered);
        let last = UTC + 199_000;
        for (live, kept) in [(last + 60_000, true), (last + CONTINUE_MS + 1, false)] {
            let mut crumbs = vec![Crumb::default(); 256];
            let mut breadcrumb = Breadcrumb::new(&mut crumbs);
            let mut rebuild = Rebuild::new(1, slots.len());
            while !rebuild.caught_up(slots.len()) {
                rebuild.step(&mut breadcrumb, slots.len(), 16, |i| slots.get(i).copied());
            }
            assert!(breadcrumb.crumbs().len() >= 2);
            rebuild.settle(&mut breadcrumb, Some(live));
            assert!(rebuild.step(&mut breadcrumb, slots.len(), 16, |_| None));
            assert_eq!(breadcrumb.crumbs().len() >= 2, kept, "live fix at {live}");
        }
    }

    #[test]
    fn a_course_is_placed_once_read_back_and_followed_live() {
        let (before, _, slots) = restarted();
        let course: Vec<course::Point> = before[..380]
            .iter()
            .map(|&(lat, lon)| course::Point::new(lat, lon).unwrap())
            .collect();
        let (mut crumbs, mut line) = (vec![Crumb::default(); 512], vec![Crumb::default(); 512]);
        let mut breadcrumb = Breadcrumb::new(&mut crumbs);
        let mut follower = Follower::new(&mut line, &mut []);
        let place = |follower: &mut Follower, frame| {
            follower.handle(Input::Course {
                points: &course,
                frame,
                read: &mut |_| None,
            });
        };
        place(&mut follower, breadcrumb.frame());
        assert!(follower.unplaced());
        let mut rebuild = Rebuild::new(1, slots.len());
        while !rebuild.step(&mut breadcrumb, slots.len(), 16, |i| slots.get(i).copied()) {
            if follower.unplaced() && breadcrumb.frame().is_some() {
                place(&mut follower, breadcrumb.frame());
            }
        }
        breadcrumb.end_replay();
        assert_eq!(
            follower.course().len(),
            course.len(),
            "placed in the ride's frame"
        );
        let mut events = Vec::new();
        for (i, &fix) in square(3)[0..40].iter().enumerate() {
            let now = (i as u64 + 1) * 1_000;
            events.extend(follower.handle(Input::Observe {
                now,
                fix: breadcrumb.observe(taken(fix, now), now),
                speed_mm_s: None,
                paused: false,
            }));
        }
        assert!(events.contains(&Event::Following), "{events:?}");
        assert!(follower.following());
    }
}
