//! How long composition rests after a pass. A pass polls every input and
//! costs the same few milliseconds whether or not anything changed, so a
//! still page rests up to `STILL_REST_MS`, bounded by the next frame and
//! harness deadline; anything that needs passes keeps `BUSY_REST_MS`. On
//! the C606 a button or touch edge ends a rest at once. Bytes from the
//! terminal's host do not: the console polls USB, so a command sent during
//! a still rest waits for its end, up to `STILL_REST_MS`.
use super::Runtime;
use super::presentation::Display;
use crate::ride::log::{Commit, Sector, Slot};
use crate::ride::storage::RideStorage;
use crate::screens::workout::Page;
use device_api::bulk::{Error, OwnedInfo, ReadWrite, SECTOR_SIZE};
use device_api::display::Display as Panel;
use device_api::input::InputSource;
use device_api::power::{Power, State};
use device_api::storage::OwnedFlash;
use firmware_shell::shell::Shell;

/// The pace while something needs passes: a ride, a finger, a host.
pub const BUSY_REST_MS: u64 = 5;
/// The longest rest on a still page. Anything without a deadline or an
/// input edge, such as a toast's end or radio data, is this late at most.
pub const STILL_REST_MS: u64 = 50;
/// Five passes a second stay under the trace's flag for a task that wakes
/// ten times a second or more while the screen is dark.
pub const STANDBY_DARK_PASS_MS: u64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pace {
    Busy,
    Still,
    Dark,
}

/// What the rest depends on, read from the owners after each pass.
struct Seen {
    power: State,
    standby_dark: bool,
    /// The terminal's host sent or took a byte since the last pass.
    host: bool,
    page: Page,
    /// Work on the page needs passes (`Runtime::busy`).
    busy: bool,
}

fn pace(seen: &Seen) -> Pace {
    if seen.host {
        return Pace::Busy;
    }
    match seen.power {
        State::Idle => {}
        State::Charging if seen.standby_dark => return Pace::Dark,
        _ => return Pace::Busy,
    }
    let still = match seen.page {
        Page::Home
        | Page::Sensors
        | Page::Settings
        | Page::Bests
        | Page::Form
        | Page::Workouts
        | Page::History
        | Page::PastRide => true,
        Page::Boot
        | Page::Ready
        | Page::Ride
        | Page::EndRide
        | Page::DiscardRide
        | Page::Summary
        | Page::Phone
        | Page::Setup => false,
    };
    if still && !seen.busy {
        Pace::Still
    } else {
        Pace::Busy
    }
}

/// `due` is the earliest deadline a still page has; one already past
/// keeps the busy pace.
fn rest_ms(pace: Pace, due: Option<u64>, now: u64) -> u64 {
    match pace {
        Pace::Busy => BUSY_REST_MS,
        Pace::Dark => STANDBY_DARK_PASS_MS,
        Pace::Still => due.map_or(STILL_REST_MS, |due| {
            due.saturating_sub(now).clamp(BUSY_REST_MS, STILL_REST_MS)
        }),
    }
}

impl Runtime {
    /// How long composition rests after a pass: `power` is the state the
    /// pass froze, `host` whether the terminal's host sent or took a byte
    /// since the last pass, and `now` the clock once the pass is done. Only
    /// reads, so it changes nothing a pass or its replay sees.
    pub fn pass_rest_ms<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &self,
        system: &Shell<D, I, P, B>,
        power: State,
        host: bool,
        now: u64,
    ) -> u64 {
        let seen = Seen {
            power,
            standby_dark: self.standby_dark(),
            host,
            page: self.navigation.page,
            busy: self.busy(system, now),
        };
        let frame = match self.display {
            Display::Active { next_frame_ms } => Some(next_frame_ms),
            Display::Suspended => None,
        };
        let due = frame.into_iter().chain(system.next_due_ms()).min();
        rest_ms(pace(&seen), due, now)
    }

    /// Work whose speed is the pass rate, or that waits on a pass soon.
    fn busy<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &self,
        system: &Shell<D, I, P, B>,
        now: u64,
    ) -> bool {
        self.recording()
            || self.navigation.touches.down()
            || self
                .page_list()
                .is_some_and(|list| self.kinetic(list).moving())
            || self.power.view().is_some()
            || self.cues.busy()
            || self.phone.busy()
            || self.discovery.searching()
            || system.frame_wanted(now)
            || self.storage_worked
            || self.terrain_status() == Some(terrain::Status::Working)
    }
}

/// Storage handed to a pass's steps, noting whether any read or wrote it:
/// a step with a backlog keeps the busy pace without a list of steps.
pub(super) struct Touched<'a, M: ?Sized> {
    media: &'a mut M,
    pub(super) touched: bool,
}

impl<'a, M: ?Sized> Touched<'a, M> {
    pub(super) fn new(media: &'a mut M) -> Self {
        Self {
            media,
            touched: false,
        }
    }
}

impl<M: RideStorage + ?Sized> RideStorage for Touched<'_, M> {
    type Error = M::Error;
    fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), M::Error> {
        self.touched = true;
        self.media.ride_read_sector(sector, output)
    }
    fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), M::Error> {
        self.touched = true;
        self.media.ride_read_slot(slot, output)
    }
    fn ride_erase_sector(&mut self, sector: usize) -> Result<(), M::Error> {
        self.touched = true;
        self.media.ride_erase_sector(sector)
    }
    fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), M::Error> {
        self.touched = true;
        self.media.ride_write_slot(slot, data)
    }
    fn ride_commit_slot(&mut self, slot: usize, commit: &Commit) -> Result<(), M::Error> {
        self.touched = true;
        self.media.ride_commit_slot(slot, commit)
    }
}

impl<M: ReadWrite + ?Sized> ReadWrite for Touched<'_, M> {
    fn owned_info(&mut self) -> Result<OwnedInfo, Error> {
        self.media.owned_info()
    }
    fn owned_read(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error> {
        self.touched = true;
        self.media.owned_read(sector, output)
    }
    fn owned_write(&mut self, sector: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), Error> {
        self.touched = true;
        self.media.owned_write(sector, input)
    }
    fn entropy(&mut self) -> u64 {
        self.media.entropy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STILL_PAGES: [Page; 8] = [
        Page::Home,
        Page::Sensors,
        Page::Settings,
        Page::Bests,
        Page::Form,
        Page::Workouts,
        Page::History,
        Page::PastRide,
    ];
    const BUSY_PAGES: [Page; 8] = [
        Page::Boot,
        Page::Ready,
        Page::Ride,
        Page::EndRide,
        Page::DiscardRide,
        Page::Summary,
        Page::Phone,
        Page::Setup,
    ];

    fn seen(page: Page) -> Seen {
        Seen {
            power: State::Idle,
            standby_dark: false,
            host: false,
            page,
            busy: false,
        }
    }

    #[test]
    fn a_page_rests_only_while_still_and_nothing_needs_a_pass() {
        for page in STILL_PAGES {
            assert_eq!(pace(&seen(page)), Pace::Still, "{page:?}");
            let busy = Seen {
                busy: true,
                ..seen(page)
            };
            assert_eq!(pace(&busy), Pace::Busy, "{page:?} with work");
            let host = Seen {
                host: true,
                ..seen(page)
            };
            assert_eq!(pace(&host), Pace::Busy, "{page:?} with a host");
        }
        for page in BUSY_PAGES {
            assert_eq!(pace(&seen(page)), Pace::Busy, "{page:?}");
        }
    }

    #[test]
    fn only_running_power_rests_and_dark_standby_rests_longest() {
        let dark = Seen {
            power: State::Charging,
            standby_dark: true,
            ..seen(Page::Home)
        };
        assert_eq!(pace(&dark), Pace::Dark);
        let hosted = Seen { host: true, ..dark };
        assert_eq!(pace(&hosted), Pace::Busy, "a host keeps the usual pace");
        let lit = Seen {
            standby_dark: false,
            ..dark
        };
        assert_eq!(pace(&lit), Pace::Busy, "lit standby animates");
        for power in [
            State::Initializing,
            State::Requested,
            State::Quiescing,
            State::Submitted,
            State::Sleeping,
            State::Recovering,
            State::Completed,
            State::Failed,
            State::Uncertain,
        ] {
            let seen = Seen {
                power,
                standby_dark: true,
                ..seen(Page::Home)
            };
            assert_eq!(pace(&seen), Pace::Busy, "{power:?}");
        }
    }

    #[test]
    fn a_still_rest_ends_by_the_next_deadline() {
        let now = 10_000;
        for (due, rest) in [
            (None, STILL_REST_MS),
            (Some(now + 1), BUSY_REST_MS),
            (Some(now + 20), 20),
            (Some(now + 1000), STILL_REST_MS),
            (Some(now), BUSY_REST_MS),
            (Some(0), BUSY_REST_MS),
        ] {
            assert_eq!(rest_ms(Pace::Still, due, now), rest, "{due:?}");
        }
        assert_eq!(rest_ms(Pace::Busy, None, now), BUSY_REST_MS);
        assert_eq!(
            rest_ms(Pace::Dark, Some(now + 20), now),
            STANDBY_DARK_PASS_MS
        );
    }
}
