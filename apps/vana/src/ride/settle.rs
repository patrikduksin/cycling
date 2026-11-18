//! The save protocol, the only code that writes FILES after a recorder result.
//! A discarded ride is hidden instead of saved. Any other ride, and a discard
//! that could not be hidden, saves its bests, workout record and facts, in that
//! order.
use crate::clock::Zone;
use crate::kept::CardRefused;
use crate::kept::bests::{self, Found, Outcome};
use crate::kept::deleted::{self, Reason, Record as Hiding};
use crate::kept::facts::record::{Climbing, Facts, Record as FactsRecord};
use crate::kept::facts::store as facts;
use crate::ride::log::Summary;
use crate::ride::medium::Medium;
use crate::ride::recovery::Reading;
use crate::ride::totals::{Efforts, Totals};
use crate::ride::zones::TrainingProfile;
use crate::workout::player::Player;
use device_api::bulk::ReadWrite;
use firmware_services::why;

#[derive(Clone, Copy)]
pub(crate) struct Saved<'a> {
    pub(crate) medium: Medium,
    pub(crate) summary: Option<Summary>,
    pub(crate) store_id: u64,
    pub(crate) totals: &'a Totals,
    pub(crate) efforts: &'a Efforts,
    pub(crate) climbing: Climbing,
    pub(crate) active_ms: u64,
    pub(crate) player: Option<&'a Player>,
    pub(crate) zone: Zone,
    pub(crate) profile: TrainingProfile,
    pub(crate) recovery: Option<Reading>,
    pub(crate) utc_ms: Option<u64>,
    pub(crate) now: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum End<'a> {
    Finished(Saved<'a>),
    Discarded(Saved<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hide {
    NotAsked,
    Hidden,
    Kept,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Claimed {
    pub(crate) found: Found,
    pub(crate) ftp: Option<u16>,
}

impl Claimed {
    fn and(self, later: Self) -> Self {
        let (earlier, found) = (self.found, later.found);
        let mut power = earlier.power;
        for (set, set_later) in power.iter_mut().zip(found.power) {
            *set |= set_later;
        }
        let found = Found {
            power,
            distance: earlier.distance || found.distance,
            time: earlier.time || found.time,
            ascent: earlier.ascent || found.ascent,
        };
        Self {
            found,
            ftp: later.ftp.or(self.ftp),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settled {
    pub(crate) hide: Hide,
    pub(crate) claimed: Option<Claimed>,
}

#[inline(never)]
pub(crate) fn settle(
    ends: [Option<End<'_>>; 2],
    bests: &mut bests::Kept,
    facts: &mut facts::Kept,
    deleted: &mut deleted::Hidden,
    bulk: &mut impl ReadWrite,
) -> Settled {
    let mut settled = Settled {
        hide: Hide::NotAsked,
        claimed: None,
    };
    for end in ends.into_iter().flatten() {
        let ride = match end {
            End::Finished(ride) => ride,
            End::Discarded(ride) if hide(&ride, deleted, bulk) => {
                why!("settle.hidden", medium = ride.medium, ride = ride_id(&ride));
                settled.hide = Hide::Hidden;
                continue;
            }
            End::Discarded(ride) => {
                why!(
                    "settle.discard_not_hidden",
                    medium = ride.medium,
                    ride = ride_id(&ride)
                );
                settled.hide = Hide::Kept;
                ride
            }
        };
        let claimed = save(&ride, bests, facts, bulk);
        why!(
            "settle.saved",
            medium = ride.medium,
            ride = ride_id(&ride),
            workout = ride.player.is_some(),
            claimed
        );
        settled.claimed = Some(
            settled
                .claimed
                .map_or(claimed, |earlier| earlier.and(claimed)),
        );
    }
    settled
}

/// `#[inline(never)]` keeps the FILES buffers out of the callers' frames.
#[inline(never)]
pub(crate) fn reset(
    medium: Medium,
    facts: &mut facts::Kept,
    deleted: &mut deleted::Hidden,
    bulk: &mut impl ReadWrite,
    now: u64,
) -> Result<(), CardRefused> {
    let history = crate::workout::history::forget(bulk, medium.journal()).or_else(|error| {
        log::warn!(target: "sdk.workout", "history_forget_failed error={}", error.name());
        crate::kept::refusal(error)
    });
    let facts = facts.forget(bulk, medium);
    let deleted = deleted.forget(bulk, medium, now);
    why!("settle.reset", medium, history, facts, deleted);
    history.and(facts).and(deleted)
}

fn ride_id(ride: &Saved<'_>) -> Option<u32> {
    ride.summary.map(|summary| summary.ride_id)
}

fn hide(ride: &Saved<'_>, deleted: &mut deleted::Hidden, bulk: &mut impl ReadWrite) -> bool {
    let Some(summary) = ride.summary else {
        return false;
    };
    let record = Hiding {
        ride: deleted::Ride::new(
            ride.medium.journal(),
            ride.store_id,
            summary.ride_id,
            summary.first_utc_ms,
            summary.active_ms,
        ),
        reason: Reason::Discarded,
        hidden_utc_s: ride
            .utc_ms
            .map_or(0, |ms| u32::try_from(ms / 1000).unwrap_or(0)),
    };
    deleted.save(bulk, &record).is_ok()
}

fn save(
    ride: &Saved<'_>,
    bests: &mut bests::Kept,
    facts: &mut facts::Kept,
    bulk: &mut impl ReadWrite,
) -> Claimed {
    let claimed = claim(ride, bests, bulk);
    if let (Some(summary), Some(player)) = (ride.summary, ride.player) {
        let record = crate::workout::history::Record {
            journal: ride.medium.journal(),
            ride_id: summary.ride_id,
            started_utc_ms: summary.first_utc_ms,
            name: player.workout().name,
            test: player.workout().test,
            summary: player.summary(),
        };
        if let Err(error) = crate::workout::history::save(bulk, &record) {
            log::warn!(target: "sdk.workout", "history_failed error={}", error.name());
        }
    }
    if let Some(record) = facts_of(ride) {
        facts.ride_saved(bulk, record, ride.recovery);
    }
    claimed
}

fn claim(ride: &Saved<'_>, bests: &mut bests::Kept, bulk: &mut impl ReadWrite) -> Claimed {
    let Some((outcome, day)) = outcome(ride) else {
        return Claimed::default();
    };
    let found = bests.merge(bulk, outcome, day, ride.now);
    let ftp = found.power[3].then(|| bests.best_20_min());
    Claimed { found, ftp }
}

fn outcome(ride: &Saved<'_>) -> Option<(Outcome, u16)> {
    let saved = ride.summary?;
    let day = saved.first_utc_ms.map_or(0, |utc| {
        (ride.zone.local(utc / 1000).0 / 86_400).min(u64::from(u16::MAX)) as u16
    });
    let clamp = |value: u64| value.min(u64::from(u32::MAX)) as u32;
    let outcome = Outcome {
        power: ride.efforts.best(),
        distance_m: clamp(ride.totals.distance_mm() / 1000),
        time_s: clamp(ride.active_ms / 1000),
        ascent_m: (ride.totals.ascent_dm() + 5) / 10,
    };
    Some((outcome, day))
}

fn facts_of(ride: &Saved<'_>) -> Option<FactsRecord> {
    let saved = ride.summary?;
    Some(FactsRecord {
        journal: ride.medium.journal(),
        ride_id: saved.ride_id,
        started_utc_ms: saved.first_utc_ms?,
        facts: Facts {
            profile: Some(ride.profile),
            recovery_bpm: None,
            climbing: Some(ride.climbing),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kept::deleted::{SHELF_BYTES, Seen, Shelf};
    use crate::workout::library::THRESHOLD;
    use crate::workout::library::tests::{Region, formatted};
    use device_api::bulk::SECTOR_SIZE;
    use firmware_services::{files, volume};
    use std::borrow::ToOwned;
    use std::boxed::Box;
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    const START_UTC_MS: u64 = 1_763_000_000_000;
    const FIRST: Option<Claimed> = Some(Claimed {
        found: Found {
            time: true,
            ..Found::EMPTY
        },
        ftp: None,
    });

    struct Ride {
        totals: Totals,
        efforts: Efforts,
        player: Player,
    }

    impl Ride {
        fn new() -> Self {
            Self {
                totals: Totals::default(),
                efforts: Efforts::default(),
                player: Player::new(THRESHOLD),
            }
        }

        fn on(&self, medium: Medium) -> Saved<'_> {
            let mut summary = Summary::started(ride_id(medium));
            summary.first_utc_ms = Some(START_UTC_MS);
            summary.active_ms = 3_600_000;
            Saved {
                medium,
                summary: Some(summary),
                store_id: 0,
                totals: &self.totals,
                efforts: &self.efforts,
                climbing: Climbing::default(),
                active_ms: 3_600_000,
                player: Some(&self.player),
                zone: Zone::Chile,
                profile: TrainingProfile::default(),
                recovery: None,
                utc_ms: Some(START_UTC_MS + 3_600_000),
                now: 0,
            }
        }
    }

    fn ride_id(medium: Medium) -> u32 {
        match medium {
            Medium::Internal => 3,
            Medium::Bulk => 8,
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum File {
        Deleted,
        Bests,
        Workout(Medium),
        Facts,
    }

    impl File {
        fn name(self) -> String {
            match self {
                Self::Deleted => "rides/deleted.tbl".into(),
                Self::Bests => "bests.dat".into(),
                Self::Workout(medium) => {
                    crate::workout::history::file(medium.journal(), ride_id(medium))
                        .as_str()
                        .into()
                }
                Self::Facts => "rides/facts.tbl".into(),
            }
        }
    }

    fn written(card: &mut Region) -> Vec<String> {
        let mut sector = [0; SECTOR_SIZE];
        let mut store = volume::open(card, &volume::FILES, &mut sector).unwrap();
        let dir = files::mount(&mut store, &mut sector).unwrap();
        let mut entries = Vec::new();
        dir.list(&mut store, &mut sector, |entry| {
            entries.push((entry.revision, entry.name.as_str().to_owned()));
        })
        .unwrap();
        entries.sort();
        entries.into_iter().map(|(_, name)| name).collect()
    }

    fn run(ends: [Option<End<'_>>; 2], hides: bool) -> (Settled, Vec<String>) {
        let mut card = formatted();
        let mut deleted = deleted::Hidden::default();
        if hides {
            let shelf = Box::leak(vec![0; SHELF_BYTES].into_boxed_slice());
            deleted.attach(Shelf::new(shelf));
            let seen = Seen {
                ready: true,
                busy: false,
                asking: false,
            };
            deleted.scan_step(&mut card, 0, seen);
        }
        let settled = settle(
            ends,
            &mut bests::Kept::default(),
            &mut facts::Kept::default(),
            &mut deleted,
            &mut card,
        );
        (settled, written(&mut card))
    }

    fn finished(ride: Saved<'_>) -> [Option<End<'_>>; 2] {
        let end = Some(End::Finished(ride));
        match ride.medium {
            Medium::Internal => [end, None],
            Medium::Bulk => [None, end],
        }
    }

    #[derive(Clone, Copy)]
    enum How {
        Finish,
        Discard,
    }

    struct Row {
        name: &'static str,
        internal: Option<How>,
        bulk: Option<How>,
        hides: bool,
        hide: Hide,
        files: &'static [File],
        claimed: Option<Claimed>,
    }

    const SAVES_INTERNAL: &[File] = &[File::Bests, File::Workout(Medium::Internal), File::Facts];
    const SAVES_BOTH: &[File] = &[
        File::Bests,
        File::Workout(Medium::Internal),
        File::Workout(Medium::Bulk),
        File::Facts,
    ];

    #[test]
    fn settle_hides_and_saves_each_medium_in_turn() {
        use How::{Discard, Finish};
        use Medium::{Bulk, Internal};
        let rows = [
            Row {
                name: "nothing ended",
                internal: None,
                bulk: None,
                hides: true,
                hide: Hide::NotAsked,
                files: &[],
                claimed: None,
            },
            Row {
                name: "a finish",
                internal: Some(Finish),
                bulk: None,
                hides: true,
                hide: Hide::NotAsked,
                files: SAVES_INTERNAL,
                claimed: FIRST,
            },
            Row {
                name: "a hidden discard",
                internal: Some(Discard),
                bulk: None,
                hides: true,
                hide: Hide::Hidden,
                files: &[File::Deleted],
                claimed: None,
            },
            Row {
                name: "a discard the table refuses",
                internal: Some(Discard),
                bulk: None,
                hides: false,
                hide: Hide::Kept,
                files: SAVES_INTERNAL,
                claimed: FIRST,
            },
            Row {
                name: "a hidden discard beside a finish",
                internal: Some(Discard),
                bulk: Some(Finish),
                hides: true,
                hide: Hide::Hidden,
                files: &[File::Deleted, File::Bests, File::Workout(Bulk), File::Facts],
                claimed: FIRST,
            },
            Row {
                name: "a finish beside a hidden discard",
                internal: Some(Finish),
                bulk: Some(Discard),
                hides: true,
                hide: Hide::Hidden,
                files: &[
                    File::Bests,
                    File::Workout(Internal),
                    File::Facts,
                    File::Deleted,
                ],
                claimed: FIRST,
            },
            Row {
                name: "both media finish",
                internal: Some(Finish),
                bulk: Some(Finish),
                hides: true,
                hide: Hide::NotAsked,
                files: SAVES_BOTH,
                claimed: FIRST,
            },
            Row {
                name: "a refused discard beside a finish",
                internal: Some(Discard),
                bulk: Some(Finish),
                hides: false,
                hide: Hide::Kept,
                files: SAVES_BOTH,
                claimed: FIRST,
            },
        ];
        let ride = Ride::new();
        let end = |medium, how| {
            let saved = ride.on(medium);
            match how {
                How::Finish => End::Finished(saved),
                How::Discard => End::Discarded(saved),
            }
        };
        for row in rows {
            let ends = [
                row.internal.map(|how| end(Internal, how)),
                row.bulk.map(|how| end(Bulk, how)),
            ];
            let (settled, written) = run(ends, row.hides);
            let expected = Settled {
                hide: row.hide,
                claimed: row.claimed,
            };
            assert_eq!(settled, expected, "{}", row.name);
            let files: Vec<String> = row.files.iter().map(|file| file.name()).collect();
            assert_eq!(written, files, "{}: what is written, in order", row.name);
        }
    }

    #[test]
    fn what_two_saves_set_adds_up() {
        let steady = Ride::new();
        let mut hard = Ride::new();
        for _ in 0..20 * 60 {
            hard.efforts.add(250);
        }
        let ends = [
            Some(End::Finished(steady.on(Medium::Internal))),
            Some(End::Finished(hard.on(Medium::Bulk))),
        ];
        let claimed = run(ends, true).0.claimed.unwrap();
        assert!(claimed.found.time, "the first ride's best stays");
        assert!(claimed.found.power[3], "the second ride's best is added");
        assert_eq!(claimed.ftp, Some(250));
    }

    #[test]
    fn a_ride_without_a_summary_is_kept_but_writes_nothing() {
        let ride = Ride::new();
        let blank = Saved {
            summary: None,
            ..ride.on(Medium::Internal)
        };
        let (settled, written) = run([Some(End::Discarded(blank)), None], true);
        let expected = Settled {
            hide: Hide::Kept,
            claimed: Some(Claimed::default()),
        };
        assert_eq!(settled, expected);
        assert!(written.is_empty());
    }

    #[test]
    fn an_undated_ride_keeps_no_facts_and_counts_on_day_zero() {
        let ride = Ride::new();
        let mut undated = ride.on(Medium::Bulk);
        if let Some(summary) = &mut undated.summary {
            summary.first_utc_ms = None;
        }
        let (settled, written) = run(finished(undated), true);
        assert_eq!(settled.claimed, FIRST);
        let files = [File::Bests, File::Workout(Medium::Bulk)].map(File::name);
        assert_eq!(written, files);
    }

    #[test]
    fn a_new_twenty_minute_best_is_offered_as_ftp() {
        let mut ride = Ride::new();
        for _ in 0..20 * 60 {
            ride.efforts.add(250);
        }
        let (settled, _) = run(finished(ride.on(Medium::Internal)), true);
        let claimed = settled.claimed.unwrap();
        assert!(claimed.found.power[3]);
        assert_eq!(claimed.ftp, Some(250));
    }

    #[test]
    fn a_ride_without_a_workout_keeps_no_workout_record() {
        let ride = Ride::new();
        let free = Saved {
            player: None,
            ..ride.on(Medium::Internal)
        };
        let (_, written) = run(finished(free), true);
        assert_eq!(written, [File::Bests, File::Facts].map(File::name));
    }
}
