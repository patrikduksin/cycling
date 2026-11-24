//! The save protocol, the only code that writes FILES after a recorder result.
use crate::kept::CardRefused;
use crate::ride::log::Summary;
use crate::ride::medium::Medium;
use crate::workout::player::Player;
use device_api::bulk::ReadWrite;
use firmware_services::why;

#[derive(Clone, Copy)]
pub(crate) struct Saved<'a> {
    pub(crate) medium: Medium,
    pub(crate) summary: Option<Summary>,
    pub(crate) player: Option<&'a Player>,
}

#[inline(never)]
pub(crate) fn settle(ends: [Option<Saved<'_>>; 2], bulk: &mut impl ReadWrite) {
    for ride in ends.into_iter().flatten() {
        save(&ride, bulk);
        why!(
            "settle.saved",
            medium = ride.medium,
            ride = ride.summary.map(|summary| summary.ride_id),
            workout = ride.player.is_some()
        );
    }
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

fn save(ride: &Saved<'_>, bulk: &mut impl ReadWrite) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workout::fixtures::THRESHOLD;
    use crate::workout::today::tests::{Region, formatted};
    use device_api::bulk::SECTOR_SIZE;
    use firmware_services::{files, volume};
    use std::borrow::ToOwned;
    use std::string::String;
    use std::vec::Vec;

    const START_UTC_MS: u64 = 1_763_000_000_000;

    struct Ride {
        player: Player,
    }

    impl Ride {
        fn new() -> Self {
            Self {
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

    fn file(medium: Medium) -> String {
        crate::workout::history::file(medium.journal(), ride_id(medium))
            .as_str()
            .into()
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

    fn run(ends: [Option<Saved<'_>>; 2]) -> Vec<String> {
        let mut card = formatted();
        settle(ends, &mut card);
        written(&mut card)
    }

    #[test]
    fn settle_saves_each_medium_in_turn() {
        let ride = Ride::new();
        let internal = Some(ride.on(Medium::Internal));
        let bulk = Some(ride.on(Medium::Bulk));
        assert!(run([None, None]).is_empty(), "nothing ended");
        assert_eq!(run([internal, None]), [file(Medium::Internal)]);
        assert_eq!(
            run([internal, bulk]),
            [file(Medium::Internal), file(Medium::Bulk)],
            "both media finish, in order"
        );
    }

    #[test]
    fn a_ride_without_a_summary_writes_nothing() {
        let ride = Ride::new();
        let blank = Saved {
            summary: None,
            ..ride.on(Medium::Internal)
        };
        assert!(run([Some(blank), None]).is_empty());
    }

    #[test]
    fn an_undated_ride_still_saves_its_workout() {
        let ride = Ride::new();
        let mut undated = ride.on(Medium::Bulk);
        if let Some(summary) = &mut undated.summary {
            summary.first_utc_ms = None;
        }
        assert_eq!(run([None, Some(undated)]), [file(Medium::Bulk)]);
    }

    #[test]
    fn a_ride_without_a_workout_keeps_no_workout_record() {
        let ride = Ride::new();
        let free = Saved {
            player: None,
            ..ride.on(Medium::Internal)
        };
        assert!(run([Some(free), None]).is_empty());
    }
}
