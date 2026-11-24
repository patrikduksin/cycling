//! The save protocol, the only code that writes FILES after a recorder result.
use crate::kept::CardRefused;
use crate::ride::log::Summary;
use crate::ride::medium::Medium;
use crate::ride::totals::Efforts;
use crate::workout::player::Player;
use device_api::bulk::ReadWrite;
use firmware_services::why;

#[derive(Clone, Copy)]
pub(crate) struct Saved<'a> {
    pub(crate) medium: Medium,
    pub(crate) summary: Option<Summary>,
    pub(crate) efforts: &'a Efforts,
    pub(crate) player: Option<&'a Player>,
}

#[derive(Clone, Copy)]
pub(crate) enum End<'a> {
    Finished(Saved<'a>),
    Discarded(Saved<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hide {
    NotAsked,
    Kept,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Claimed {
    pub(crate) ftp: Option<u16>,
}

impl Claimed {
    fn and(self, later: Self) -> Self {
        Self {
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
pub(crate) fn settle(ends: [Option<End<'_>>; 2], bulk: &mut impl ReadWrite) -> Settled {
    let mut settled = Settled {
        hide: Hide::NotAsked,
        claimed: None,
    };
    for end in ends.into_iter().flatten() {
        let ride = match end {
            End::Finished(ride) => ride,
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
        let claimed = save(&ride, bulk);
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
pub(crate) fn reset(medium: Medium, bulk: &mut impl ReadWrite) -> Result<(), CardRefused> {
    let history = crate::workout::history::forget(bulk, medium.journal()).or_else(|error| {
        log::warn!(target: "sdk.workout", "history_forget_failed error={}", error.name());
        crate::kept::refusal(error)
    });
    why!("settle.reset", medium, history);
    history
}

fn ride_id(ride: &Saved<'_>) -> Option<u32> {
    ride.summary.map(|summary| summary.ride_id)
}

fn save(ride: &Saved<'_>, bulk: &mut impl ReadWrite) -> Claimed {
    let claimed = Claimed {
        ftp: ride.summary.and(ride.efforts.best()[3]),
    };
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
    claimed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workout::library::THRESHOLD;
    use crate::workout::library::tests::{Region, formatted};
    use device_api::bulk::SECTOR_SIZE;
    use firmware_services::{files, volume};
    use std::borrow::ToOwned;
    use std::string::String;
    use std::vec::Vec;

    const START_UTC_MS: u64 = 1_763_000_000_000;
    const FIRST: Option<Claimed> = Some(Claimed { ftp: None });

    struct Ride {
        efforts: Efforts,
        player: Player,
    }

    impl Ride {
        fn new() -> Self {
            Self {
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
                efforts: &self.efforts,
                player: Some(&self.player),
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
        Workout(Medium),
    }

    impl File {
        fn name(self) -> String {
            match self {
                Self::Workout(medium) => {
                    crate::workout::history::file(medium.journal(), ride_id(medium))
                        .as_str()
                        .into()
                }
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

    fn run(ends: [Option<End<'_>>; 2]) -> (Settled, Vec<String>) {
        let mut card = formatted();
        let settled = settle(ends, &mut card);
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
        hide: Hide,
        files: &'static [File],
        claimed: Option<Claimed>,
    }

    const SAVES_INTERNAL: &[File] = &[File::Workout(Medium::Internal)];
    const SAVES_BOTH: &[File] = &[File::Workout(Medium::Internal), File::Workout(Medium::Bulk)];

    #[test]
    fn settle_saves_each_medium_in_turn() {
        use How::{Discard, Finish};
        let rows = [
            Row {
                name: "nothing ended",
                internal: None,
                bulk: None,
                hide: Hide::NotAsked,
                files: &[],
                claimed: None,
            },
            Row {
                name: "a finish",
                internal: Some(Finish),
                bulk: None,
                hide: Hide::NotAsked,
                files: SAVES_INTERNAL,
                claimed: FIRST,
            },
            Row {
                name: "a discard",
                internal: Some(Discard),
                bulk: None,
                hide: Hide::Kept,
                files: SAVES_INTERNAL,
                claimed: FIRST,
            },
            Row {
                name: "both media finish",
                internal: Some(Finish),
                bulk: Some(Finish),
                hide: Hide::NotAsked,
                files: SAVES_BOTH,
                claimed: FIRST,
            },
            Row {
                name: "a discard beside a finish",
                internal: Some(Discard),
                bulk: Some(Finish),
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
                row.internal.map(|how| end(Medium::Internal, how)),
                row.bulk.map(|how| end(Medium::Bulk, how)),
            ];
            let (settled, written) = run(ends);
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
    fn the_twenty_minute_best_of_either_save_is_offered() {
        let steady = Ride::new();
        let mut hard = Ride::new();
        for _ in 0..20 * 60 {
            hard.efforts.add(250);
        }
        let ends = [
            Some(End::Finished(steady.on(Medium::Internal))),
            Some(End::Finished(hard.on(Medium::Bulk))),
        ];
        let claimed = run(ends).0.claimed.unwrap();
        assert_eq!(claimed.ftp, Some(250));
    }

    #[test]
    fn a_ride_without_a_summary_is_kept_but_writes_nothing() {
        let ride = Ride::new();
        let blank = Saved {
            summary: None,
            ..ride.on(Medium::Internal)
        };
        let (settled, written) = run([Some(End::Discarded(blank)), None]);
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
        let (settled, written) = run(finished(undated));
        assert_eq!(settled.claimed, FIRST);
        let files = [File::Workout(Medium::Bulk)].map(File::name);
        assert_eq!(written, files);
    }

    #[test]
    fn a_twenty_minute_best_is_offered_as_ftp() {
        let mut ride = Ride::new();
        for _ in 0..20 * 60 {
            ride.efforts.add(250);
        }
        let (settled, _) = run(finished(ride.on(Medium::Internal)));
        assert_eq!(settled.claimed.unwrap().ftp, Some(250));
    }

    #[test]
    fn a_ride_without_a_workout_keeps_no_workout_record() {
        let ride = Ride::new();
        let free = Saved {
            player: None,
            ..ride.on(Medium::Internal)
        };
        let (_, written) = run(finished(free));
        assert!(written.is_empty());
    }
}
