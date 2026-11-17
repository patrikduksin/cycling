use super::plan::Step;
use crate::rides::Ride;
use crate::sim::Sim;
use vana::ride::log::Kind;

pub struct Observation {
    pub at_ms: u64,
    pub boot: u32,
    /// When the device last came on: a boot, or a wake from charging
    /// standby, which shows the boot page again in the same session.
    pub on_since_ms: u64,
    pub page: &'static str,
    pub off: bool,
    pub journal: Vec<Ride>,
    /// Rides on the card; empty without one.
    pub card: Vec<Ride>,
    /// A storage fault was armed or fired since this boot began.
    pub faulted: bool,
}

impl Observation {
    pub fn summary(&self) -> String {
        let mut out = self.page.to_string();
        for (medium, rides) in media(self) {
            if let Some(ride) = rides.last() {
                let last = ride.kinds().last().copied();
                out += &format!(
                    " {medium}: {} rides, #{} {} samples {last:?}",
                    rides.len(),
                    ride.id,
                    ride.samples().count()
                );
            }
        }
        out
    }
}

/// Read the device. `faulted` carries over from the previous observation
/// within the same boot.
pub fn observe(sim: &mut Sim, card: bool, previous: Option<&Observation>) -> Observation {
    let (at_ms, boot, off) = (sim.now(), sim.device.boot, sim.is_off());
    let armed = sim.storage_faulted();
    let carried = previous.is_some_and(|p| p.boot == boot && p.faulted);
    let on_since_ms = match previous {
        Some(p) if p.boot == boot && !p.off => p.on_since_ms,
        Some(p) if p.boot == boot => at_ms,
        _ => at_ms.saturating_sub(sim.device.now),
    };
    Observation {
        at_ms,
        boot,
        on_since_ms,
        page: if off { "off" } else { sim.vana_page() },
        off,
        journal: sim.rides(),
        card: if card { card_rides(sim) } else { Vec::new() },
        faulted: armed || carried,
    }
}

pub(super) fn card_rides(sim: &Sim) -> Vec<Ride> {
    use vana::ride::bulk::{Bulk, BulkState};
    use vana::ride::journal::{Journal, Scan};
    let mut media = sim.device.bulk.inspect();
    let mut state = BulkState::default();
    let mut journal = Bulk::new(&mut state, &mut media);
    let catalog = loop {
        match journal.scan() {
            Scan::More => {}
            Scan::Done(catalog) | Scan::Uncertain(catalog) => break catalog,
            _ => return Vec::new(),
        }
    };
    let mut bytes = Vec::new();
    for index in 0..catalog.next_slot {
        let mut slot = vana::ride::log::Slot::default();
        if journal.read_slot(index, &mut slot) == Some(true) {
            bytes.extend_from_slice(&slot.0);
        }
    }
    crate::rides::read(&bytes)
}

pub enum Verdict {
    Pass,
    /// The check applied, but a guard named here kept it from judging.
    Skip(&'static str),
    Fail(String),
}

/// One step and the device before and after it; `step` is `None` for the
/// end of the session.
pub struct Moment<'a> {
    pub step: Option<&'a Step>,
    pub before: &'a Observation,
    pub after: &'a Observation,
}

pub struct Check {
    pub name: &'static str,
    pub run: fn(&Moment) -> Verdict,
}

pub const CHECKS: &[Check] = &[
    Check {
        name: "leaves the boot page",
        run: leaves_boot,
    },
    Check {
        name: "committed samples survive",
        run: samples_survive,
    },
    Check {
        name: "rides decode",
        run: rides_decode,
    },
    Check {
        name: "no garbage sample",
        run: no_garbage,
    },
    Check {
        name: "keeps recording while riding",
        run: keeps_recording,
    },
];

pub(super) const BOOT_PAGE_DEADLINE_MS: u64 = 15_000;

fn leaves_boot(moment: &Moment) -> Verdict {
    let after = moment.after;
    let on_ms = after.at_ms - after.on_since_ms;
    if after.off || on_ms < BOOT_PAGE_DEADLINE_MS || after.page != "boot" {
        return Verdict::Pass;
    }
    Verdict::Fail(format!(
        "still on the boot page {} s after power-on",
        on_ms / 1000
    ))
}

fn media(observation: &Observation) -> [(&'static str, &[Ride]); 2] {
    [
        ("journal", &observation.journal),
        ("card", &observation.card),
    ]
}

/// Pages where a press can discard or delete a ride.
const REMOVING_PAGES: [&str; 2] = ["discard_ride", "past_ride"];

fn samples_survive(moment: &Moment) -> Verdict {
    if matches!(moment.step, Some(Step::Press(_) | Step::Hold(..)))
        && REMOVING_PAGES.contains(&moment.before.page)
    {
        return Verdict::Skip("a press on a page that removes rides");
    }
    for ((medium, before), (_, after)) in media(moment.before).into_iter().zip(media(moment.after))
    {
        for ride in before {
            let had = ride.samples().count();
            match after.iter().find(|r| r.id == ride.id) {
                None => {
                    return Verdict::Fail(format!(
                        "{medium} ride {} with {had} samples is gone",
                        ride.id
                    ));
                }
                Some(now) if now.samples().count() < had => {
                    return Verdict::Fail(format!(
                        "{medium} ride {} had {had} samples, now {}",
                        ride.id,
                        now.samples().count()
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Verdict::Pass
}

fn terminal(kind: Kind) -> bool {
    matches!(kind, Kind::Finish | Kind::Recovered | Kind::Full)
}

fn rides_decode(moment: &Moment) -> Verdict {
    for (medium, rides) in media(moment.after) {
        for (index, ride) in rides.iter().enumerate() {
            let kinds = ride.kinds();
            let newest = index + 1 == rides.len();
            let terminals = kinds.iter().filter(|k| terminal(**k)).count();
            let fault = if rides[..index].iter().any(|r| r.id == ride.id) {
                "its id appears twice"
            } else if kinds.first() != Some(&Kind::Start) {
                "it does not begin with Start"
            } else if terminals > 1
                || (terminals == 1 && !kinds.last().is_some_and(|k| terminal(*k)))
            {
                "a terminal record is not its last"
            } else if terminals == 0 && !newest {
                "a newer ride started while it was open"
            } else {
                continue;
            };
            return Verdict::Fail(format!(
                "{medium} ride {}: {fault}; kinds {kinds:?}",
                ride.id
            ));
        }
    }
    Verdict::Pass
}

const FASTEST_KMH: u32 = 150;
const FASTEST_MM_S: u32 = (FASTEST_KMH * 1_000_000).div_ceil(3_600);
const HIGHEST_BPM: u16 = 250;

fn no_garbage(moment: &Moment) -> Verdict {
    for (medium, rides) in media(moment.after) {
        for ride in rides {
            let mut last_ms = 0;
            for sample in ride.samples() {
                let fault = if sample.active_ms < last_ms {
                    "active time went back".to_string()
                } else if sample.speed_mm_s.is_some_and(|v| v > FASTEST_MM_S) {
                    format!("speed beyond {FASTEST_KMH} km/h")
                } else if sample.heart_bpm.is_some_and(|v| v > HIGHEST_BPM) {
                    format!("heart rate beyond {HIGHEST_BPM}")
                } else if sample.battery_percent.is_some_and(|v| v > 100) {
                    "battery beyond 100 %".to_string()
                } else {
                    last_ms = sample.active_ms;
                    continue;
                };
                return Verdict::Fail(format!("{medium} ride {}: {fault}: {sample:?}", ride.id));
            }
        }
    }
    Verdict::Pass
}

const JUDGED_WAIT_MS: u32 = 30_000;
/// The share of a judged wait's seconds a recording ride must gain as samples.
const SAMPLED_PERCENT: usize = 80;
/// Batches a judged wait may fall short by, since samples reach the medium a
/// batch at a time.
const BATCH_SLACK: usize = 2;

fn keeps_recording(moment: &Moment) -> Verdict {
    let Some(Step::Wait { ms }) = moment.step else {
        return Verdict::Pass;
    };
    if *ms < JUDGED_WAIT_MS {
        return Verdict::Pass;
    }
    let open = |rides: &[Ride]| {
        rides
            .last()
            .filter(|r| {
                matches!(
                    r.kinds().last(),
                    Some(Kind::Start | Kind::Samples | Kind::Resume)
                )
            })
            .map(|r| (r.id, r.samples().count(), r.entries.len()))
    };
    let (before, after) = (moment.before, moment.after);
    let (medium, (id, had, entries)) = match (open(&before.journal), open(&before.card)) {
        (Some(ride), None) => (&after.journal, ride),
        (None, Some(ride)) => (&after.card, ride),
        (None, None) => return Verdict::Pass,
        (Some(_), Some(_)) => return Verdict::Skip("open rides on both media"),
    };
    if after.boot != before.boot {
        return Verdict::Skip("rebooted during the wait");
    }
    if after.off {
        return Verdict::Skip("turned off during the wait");
    }
    if before.faulted || after.faulted {
        return Verdict::Skip("a storage fault this boot");
    }
    let Some(ride) = medium.iter().find(|r| r.id == id) else {
        return Verdict::Fail(format!("ride {id} is gone"));
    };
    let Some(new) = ride.entries.get(entries..) else {
        return Verdict::Fail(format!(
            "ride {id} shrank from {entries} entries to {}",
            ride.entries.len()
        ));
    };
    let stopped = new.iter().any(|e| e.kind != Kind::Samples);
    if stopped {
        return Verdict::Skip("paused or ended during the wait");
    }
    let gained = ride.samples().count().saturating_sub(had);
    let expected = (*ms as usize / 1000 * SAMPLED_PERCENT / 100)
        .saturating_sub(BATCH_SLACK * vana::ride::log::SAMPLES_PER_BATCH);
    if gained < expected {
        return Verdict::Fail(format!(
            "ride {id} gained {gained} samples in {} s of recording",
            ms / 1000
        ));
    }
    Verdict::Pass
}
