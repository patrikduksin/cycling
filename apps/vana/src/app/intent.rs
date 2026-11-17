//! A record outside every store names a wipe before its first write, so a
//! boot can finish a wipe a power cut left.
//!
//! A commit word cut by power can be marginal: it reads whole on one boot
//! and torn on the next. Stages therefore run only behind a commit that
//! completed. Such a commit never reads torn later, so a record dropped
//! because it reads torn had no stage behind it.

use super::format::Scope;
use device_api::storage::{OwnedFlash, Region};
use firmware_services::storage::{Error, Held, Journal};

const VERSION: u8 = 1;
const IDLE: u8 = 0;
pub const IDLE_RECORD: [u8; 2] = [VERSION, IDLE];
const SETTLE_ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    Idle,
    ForeignBytes,
    /// Either sector counts, not only the newest: a commit word left
    /// marginal by a cut can read whole on a later boot.
    Pending(Scope),
    ReadFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Settles {
    ForeignBytesOnly,
    AnyRecord,
}

/// The wipe this boot must finish, committed again so its stages may run;
/// `ReadFailed` until that commit is done.
#[inline(never)]
pub(crate) fn at_boot(flash: impl OwnedFlash) -> Found {
    let mut journal = Journal::new(flash, Region::Intent);
    let found = match found(&mut journal) {
        Found::ForeignBytes if settle_in(&mut journal, Settles::ForeignBytesOnly) => Found::Idle,
        Found::ForeignBytes => found(&mut journal),
        found => found,
    };
    match found {
        Found::Pending(_) => recommit(&mut journal).map_or(Found::ReadFailed, Found::Pending),
        found => found,
    }
}

/// Record `scope` before its wipe's first write. A save could erase a
/// pending copy, so with a wipe pending nothing new is recorded: the
/// pending wipe runs when it covers `scope`, and otherwise nothing does.
#[inline(never)]
pub(crate) fn begin(flash: impl OwnedFlash, scope: Scope) -> Option<Scope> {
    let mut journal = Journal::new(flash, Region::Intent);
    match found(&mut journal) {
        Found::ReadFailed => None,
        Found::Pending(_) => recommit(&mut journal).filter(|pending| pending.covers(scope)),
        Found::Idle | Found::ForeignBytes => {
            journal.save(&[VERSION, scope.bits()]).ok().map(|_| scope)
        }
    }
}

#[inline(never)]
pub(crate) fn settle(flash: impl OwnedFlash) -> bool {
    settle_in(&mut Journal::new(flash, Region::Intent), Settles::AnyRecord)
}

fn settle_in<F: OwnedFlash>(journal: &mut Journal<F>, settles: Settles) -> bool {
    for _ in 0..SETTLE_ATTEMPTS {
        match found(journal) {
            Found::Idle => return true,
            Found::Pending(_) | Found::ReadFailed if settles == Settles::ForeignBytesOnly => {
                return false;
            }
            Found::ForeignBytes | Found::Pending(_) | Found::ReadFailed => {}
        }
        let saved = match journal.save(&IDLE_RECORD) {
            Err(Error::UnrecognizedJournal) => journal.erase(),
            saved => saved.map(drop),
        };
        if saved.is_err() {
            return false;
        }
    }
    found(journal) == Found::Idle
}

fn recommit<F: OwnedFlash>(journal: &mut Journal<F>) -> Option<Scope> {
    let mut pending = 0;
    for sector in 0..2 {
        let mut payload = [0; 2];
        match journal.held(sector, &mut payload) {
            Ok(Held::Committed(record)) if pending_in(record.length, payload).is_some() => {
                journal.recommit(sector).ok()?;
                pending |= payload[1];
            }
            Err(Error::Flash(_)) => return None,
            _ => {}
        }
    }
    Scope::from_bits(pending)
}

fn pending_in(length: usize, payload: [u8; 2]) -> Option<Scope> {
    (length == 2 && payload[0] == VERSION)
        .then(|| Scope::from_bits(payload[1]))
        .flatten()
}

fn found<F: OwnedFlash>(journal: &mut Journal<F>) -> Found {
    let mut pending = 0;
    let mut foreign = false;
    for sector in 0..2 {
        let mut payload = [0; 2];
        match journal.held(sector, &mut payload) {
            Ok(Held::Blank) | Err(Error::UnsupportedGeometry) => {}
            Err(Error::Flash(_)) => return Found::ReadFailed,
            Ok(Held::Committed(record)) if record.length == 2 && payload == IDLE_RECORD => {}
            Ok(Held::Committed(record)) if pending_in(record.length, payload).is_some() => {
                pending |= payload[1];
            }
            _ => foreign = true,
        }
    }
    match Scope::from_bits(pending) {
        Some(scope) => Found::Pending(scope),
        None if foreign => Found::ForeignBytes,
        None => Found::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Store;
    use crate::testing::{Cut, IntentFlash, Rng};

    fn read(flash: &mut IntentFlash) -> Found {
        found(&mut Journal::new(flash, Region::Intent))
    }

    fn scope(rng: &mut Rng) -> Scope {
        let stores = Store::ALL.into_iter().filter(|_| rng.one_in(2));
        Scope::of(stores).unwrap_or(Scope::one(Store::Internal))
    }

    fn cut_soon(flash: &mut IntentFlash, rng: &mut Rng) {
        flash.cut = Some(Cut {
            at: flash.mutations + rng.below(8),
            seed: rng.next(),
        });
    }

    /// A device's intent across begins, boots and cuts, with what the wipe
    /// may and must do: `begun` holds every store a begin asked for since a
    /// settle last returned, and `ran` every store with a stage run behind
    /// the record since a settle last started.
    #[derive(Default)]
    struct Device {
        flash: IntentFlash,
        begun: u8,
        ran: u8,
    }

    impl Device {
        fn wipe(&mut self, scope: Scope, rng: &mut Rng, what: &str) {
            assert_eq!(
                scope.bits() & !self.begun,
                0,
                "{what}: a wipe nothing began"
            );
            assert!(
                scope.covers(Scope::from_bits(self.ran).unwrap_or(scope)),
                "{what}: a half-run wipe was dropped"
            );
            self.ran |= scope.bits();
            if self.flash.off || rng.one_in(4) {
                return;
            }
            self.ran = 0;
            if settle(&mut self.flash) {
                self.begun = 0;
            }
        }

        fn begin(&mut self, scope: Scope, rng: &mut Rng, what: &str) {
            self.begun |= scope.bits();
            if let Some(recorded) = begin(&mut self.flash, scope) {
                assert!(recorded.covers(scope), "{what}");
                self.wipe(recorded, rng, what);
            }
        }

        fn boot_admits_rides(&mut self, rng: &mut Rng, what: &str) -> bool {
            self.flash.power_on(rng.next());
            if rng.one_in(3) {
                cut_soon(&mut self.flash, rng);
            }
            match at_boot(&mut self.flash) {
                Found::Idle => {
                    assert_eq!(self.ran, 0, "{what}: a half-run wipe was dropped");
                    true
                }
                Found::Pending(due) => {
                    self.wipe(due, rng, what);
                    false
                }
                Found::ForeignBytes | Found::ReadFailed => false,
            }
        }

        /// A medium that refused a write abandons the wipe.
        fn abandon(&mut self) {
            self.ran = 0;
            if settle(&mut self.flash) {
                self.begun = 0;
            }
        }

        fn boot_until_rides_admitted(&mut self, rng: &mut Rng, what: &str) {
            for boots in 0.. {
                assert!(
                    boots < 100,
                    "{what}: boots go on: {:?}",
                    read(&mut self.flash)
                );
                if self.boot_admits_rides(rng, what) {
                    break;
                }
            }
            self.flash.cut = None;
            for _ in 0..4 {
                assert!(
                    !matches!(read(&mut self.flash), Found::Pending(_)),
                    "{what}: an idle boot holds"
                );
            }
        }
    }

    #[test]
    fn a_cut_at_any_write_of_a_begin_or_settle_runs_a_wipe_only_once_begun() {
        let mut flash = IntentFlash::default();
        let rides = Scope::of([Store::Rides]).unwrap();
        assert_eq!(begin(&mut flash, rides), Some(rides));
        assert!(settle(&mut flash));
        let writes = flash.mutations;
        for seed in 0..64 {
            for cut in 0..writes {
                let what = std::format!("seed {seed} cut {cut}");
                let mut rng = Rng::new(seed);
                let mut device = Device::default();
                device.flash.cut = Some(Cut {
                    at: cut,
                    seed: rng.next(),
                });
                device.begin(rides, &mut rng, &what);
                device.boot_until_rides_admitted(&mut rng, &what);
            }
        }
    }

    fn random_begins_boots_and_cuts(seed: u64) -> bool {
        let what = std::format!("seed {seed}");
        let mut rng = Rng::new(seed);
        let mut device = Device::default();
        if rng.one_in(4) {
            for byte in device.flash.bytes.iter_mut() {
                *byte = rng.byte();
            }
        }
        let mut resumed = false;
        for _ in 0..rng.range(1, 16) {
            if rng.one_in(3) {
                cut_soon(&mut device.flash, &mut rng);
            }
            if device.flash.off || rng.one_in(3) {
                resumed |= !device.boot_admits_rides(&mut rng, &what) && device.ran != 0;
            } else if rng.one_in(5) {
                device.abandon();
            } else {
                device.begin(scope(&mut rng), &mut rng, &what);
            }
        }
        device.boot_until_rides_admitted(&mut rng, &what);
        resumed
    }

    #[test]
    fn random_begins_boots_and_cuts_run_every_begun_wipe_and_drop_none_half_run() {
        let resumed = (0..1_000)
            .filter(|&seed| random_begins_boots_and_cuts(seed))
            .count();
        assert!(resumed > 50, "{resumed} seeds left a wipe half run");
    }

    /// Stages never run behind a commit a later boot can discard.
    #[test]
    fn two_cut_commits_then_a_cut_during_the_stages_leave_the_wipe_due() {
        let internal = Scope::one(Store::Internal);
        let mut half_run = 0;
        for seed in 0..512 {
            let what = std::format!("seed {seed}");
            let mut rng = Rng::new(seed);
            let mut device = Device::default();
            device.flash.cut = Some(Cut {
                at: 2,
                seed: rng.next(),
            });
            device.begin(internal, &mut rng, &what);
            for second in [true, false] {
                device.flash.power_on(rng.next());
                device.flash.cut = second.then(|| Cut {
                    at: device.flash.mutations,
                    seed: rng.next(),
                });
                if let Found::Pending(due) = at_boot(&mut device.flash) {
                    device.wipe(due, &mut rng, &what);
                    device.flash.off = true;
                }
            }
            half_run += usize::from(device.ran != 0);
            device.boot_until_rides_admitted(&mut rng, &what);
        }
        assert!(half_run > 32, "{half_run} wipes were cut half run");
    }

    /// A begin cut in its commit word can read torn, then whole: a boot
    /// that settles what it read as unreadable, and is cut doing so, must
    /// not admit rides over a wipe a later read finds due.
    #[test]
    fn a_settle_cut_at_boot_leaves_no_wipe_due_behind_a_boot_that_admits_rides() {
        let internal = Scope::one(Store::Internal);
        let mut hazards = 0;
        for seed in 0..256 {
            for settle_cut in 0..3 {
                let mut rng = Rng::new(seed);
                let mut flash = IntentFlash::default();
                flash.cut = Some(Cut {
                    at: 2,
                    seed: rng.next(),
                });
                assert_eq!(begin(&mut flash, internal), None);
                flash.power_on(rng.next() ^ settle_cut);
                flash.cut = Some(Cut {
                    at: flash.mutations + settle_cut,
                    seed: rng.next(),
                });
                let admitted = at_boot(&mut flash) == Found::Idle;
                flash.power_on(rng.next());
                let due = (0..8).any(|_| matches!(read(&mut flash), Found::Pending(_)));
                assert!(!(admitted && due), "seed {seed} settle cut {settle_cut}");
                hazards += usize::from(due);
            }
        }
        assert!(hazards > 64, "{hazards} cuts left a wipe that reads due");
    }

    #[test]
    fn a_boot_commits_a_marginal_record_again_so_every_later_read_finds_it() {
        let internal = Scope::one(Store::Internal);
        let mut resumed = 0;
        for seed in 0..256 {
            let mut rng = Rng::new(seed);
            let mut flash = IntentFlash::default();
            flash.cut = Some(Cut {
                at: 2,
                seed: rng.next(),
            });
            assert_eq!(begin(&mut flash, internal), None);
            flash.power_on(rng.next());
            if at_boot(&mut flash) != Found::Pending(internal) {
                continue;
            }
            resumed += 1;
            for _ in 0..16 {
                flash.power_on(rng.next());
                assert_eq!(read(&mut flash), Found::Pending(internal), "seed {seed}");
            }
        }
        assert!(resumed > 32, "{resumed} boots resumed");
    }

    #[test]
    fn a_begin_beside_a_pending_wipe_records_nothing_and_runs_it_only_if_it_covers() {
        let mut flash = IntentFlash::default();
        let rides = Scope::one(Store::Rides);
        let settings = Scope::one(Store::Settings);
        let both = Scope::of([Store::Rides, Store::Settings]).unwrap();
        assert_eq!(begin(&mut flash, both), Some(both));
        let recorded = flash.bytes.clone();
        assert_eq!(begin(&mut flash, rides), Some(both));
        assert_eq!(begin(&mut flash, settings), Some(both));
        assert_eq!(begin(&mut flash, Scope::one(Store::Internal)), None);
        assert!(flash.bytes == recorded, "nothing new is recorded");
        assert_eq!(at_boot(&mut flash), Found::Pending(both));
    }

    #[test]
    fn a_read_the_flash_refuses_is_no_answer_and_drops_no_wipe() {
        let mut flash = IntentFlash::default();
        let internal = Scope::one(Store::Internal);
        assert_eq!(begin(&mut flash, internal), Some(internal));
        flash.off = true;
        assert_eq!(at_boot(&mut flash), Found::ReadFailed);
        assert_eq!(begin(&mut flash, internal), None);
        flash.power_on(0);
        assert_eq!(at_boot(&mut flash), Found::Pending(internal));
    }

    #[test]
    fn bytes_this_version_cannot_read_are_no_wipe_and_settle() {
        let other_payloads: [&[u8]; 4] = [&[2, 0x20], &[VERSION, 0x80], &[VERSION, 0x20, 0], &[]];
        for (case, payload) in other_payloads.iter().enumerate() {
            let mut flash = IntentFlash::default();
            Journal::new(&mut flash, Region::Intent)
                .save(payload)
                .unwrap();
            assert_eq!(read(&mut flash), Found::ForeignBytes, "case {case}");
            assert_eq!(at_boot(&mut flash), Found::Idle, "case {case}");
            assert_eq!(read(&mut flash), Found::Idle, "case {case}");
        }
        for seed in 0..64 {
            let mut rng = Rng::new(seed);
            let mut flash = IntentFlash::default();
            let garbage = rng.pick(&[4096, 8192]);
            for byte in &mut flash.bytes[..garbage] {
                *byte = rng.byte();
            }
            assert_ne!(read(&mut flash), Found::Idle, "seed {seed}");
            assert_eq!(at_boot(&mut flash), Found::Idle, "seed {seed}");
            assert_eq!(read(&mut flash), Found::Idle, "seed {seed}");
        }
        assert_eq!(read(&mut IntentFlash::default()), Found::Idle);
    }

    #[test]
    fn a_settled_journal_is_read_without_a_write() {
        let mut flash = IntentFlash::default();
        assert_eq!(at_boot(&mut flash), Found::Idle);
        assert!(settle(&mut flash));
        assert_eq!(flash.mutations, 0);
        let settings = Scope::one(Store::Settings);
        assert_eq!(begin(&mut flash, settings), Some(settings));
        assert!(settle(&mut flash));
        let settled = flash.mutations;
        assert!(settle(&mut flash));
        assert_eq!(at_boot(&mut flash), Found::Idle);
        assert_eq!(flash.mutations, settled);
    }
}
