//! Back to start: the ride's breadcrumb, fed from fresh fixes, and nudges
//! when following it back, or when following the loaded course.
//!
//! Composition gives `BackToStart` a block of its own with room for
//! `BACK_CRUMBS` points (PSRAM on the device), the loaded course beside them
//! and room for it in the ride's frame, and hands it to
//! `Runtime::attach_back`; `Runtime` keeps only the reference, so none of it
//! costs main's stack anything. Without it the back-to-start page says NO
//! TRACK and no course can be loaded.
//!
//! The ride started after a restart that recovered one reads the recovered
//! ride's track and start back from the journal (`ride::homing::rebuild`), a
//! few slots a tick, before it takes live fixes.
use super::Runtime;
use super::cues::Cue;
use super::recording::{Medium, Ride};
use crate::ride::course::Course;
use crate::ride::homing::rebuild::Rebuild;
use crate::ride::homing::{Event, Homing};
use crate::ride::log::{Slot, Status, decode};
use crate::screens::chrome::Tone;
pub use crate::screens::ride::MapFrame;
use crate::screens::ride::{Back, MapKey, RidePage, Track, update_map};

pub use crate::ride::homing::Crumb;

/// Points the breadcrumb keeps: 48 KiB, thinned to fit a ride of any length.
pub const BACK_CRUMBS: usize = 4096;
/// The nudges stay a little longer than a confirmation.
const NUDGE_MS: u64 = 4_000;
/// Slots the rebuild reads per tick, as the history replay does.
const REBUILD_INTERNAL: usize = 32;
const REBUILD_BULK: usize = 8;

/// A ride a restart cut short, whose track the next ride picks up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resume {
    None,
    /// A restart closed ride `ride_id` on `medium` as recovered; the next
    /// ride there reads its track back.
    Waiting {
        medium: Medium,
        ride_id: u32,
    },
    /// The ride under way is reading the track back from `medium`.
    Reading {
        medium: Medium,
        rebuild: Rebuild,
    },
}

impl Resume {
    /// What the next ride on `ride`'s medium picks up, while none is open.
    fn waiting(ride: &Ride) -> Self {
        let medium = ride.medium();
        let recorder = ride.recorder(medium);
        if recorder.status() != Status::Recovered || recorder.recovering() {
            return Self::None;
        }
        let last = usize::from(recorder.summary_count()).saturating_sub(1);
        match recorder.summary(last) {
            Some(summary) if summary.recovered => Self::Waiting {
                medium,
                ride_id: summary.ride_id,
            },
            _ => Self::None,
        }
    }
}

/// The breadcrumb and whether the ride it belongs to is still open.
pub struct BackToStart {
    homing: Homing<'static>,
    riding: bool,
    /// The map page's track, which `map_of` keeps up to date.
    map: &'static mut MapFrame,
    map_key: Option<MapKey>,
    /// The loaded course and whether it matches the stored one.
    pub(super) course: &'static mut Course,
    pub(super) kept: super::course::Kept,
    resume: Resume,
}

impl BackToStart {
    /// The breadcrumb in `crumbs`, the course and room for it in the
    /// ride's frame in `line`; all may be zeroed memory.
    pub fn new(
        crumbs: &'static mut [Crumb],
        line: &'static mut [Crumb],
        map: &'static mut MapFrame,
        course: &'static mut Course,
    ) -> Self {
        Self {
            homing: Homing::new(crumbs).with_line(line),
            riding: false,
            map,
            map_key: None,
            course,
            kept: super::course::Kept::default(),
            resume: Resume::None,
        }
    }

    /// Follow the course as it is now, or nothing once it is cleared.
    pub(super) fn course_changed(&mut self) {
        self.homing.set_course(self.course.points());
    }
}

impl Runtime {
    /// Give back-to-start its breadcrumb; without one it never runs.
    pub fn attach_back(&mut self, back: &'static mut BackToStart) {
        self.back = Some(back);
    }

    /// Feed this pass's fix to the open ride's breadcrumb, and nudge on
    /// what following it back changed. A new ride starts a new breadcrumb,
    /// or reads back the one a restart cut short from `store` or `bulk`.
    pub(super) fn track_back(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        bulk: &mut impl device_api::bulk::ReadWrite,
        now: u64,
    ) {
        let Some(state) = self.back.as_deref_mut() else {
            return;
        };
        let riding = self.ride.active().riding();
        if riding && !state.riding {
            state.homing.reset();
            state.resume = match state.resume {
                Resume::Waiting { medium, ride_id } if medium == self.ride.shown() => {
                    let upper = self.ride.recorder(medium).next_slot();
                    Resume::Reading {
                        medium,
                        rebuild: Rebuild::new(ride_id, upper),
                    }
                }
                _ => Resume::None,
            };
        }
        state.riding = riding;
        state
            .homing
            .set_auto_follow(self.preferences.options.ride_pages.shows(RidePage::Back));
        if !riding {
            state.resume = Resume::waiting(&self.ride);
            return;
        }
        if let Resume::Reading {
            medium,
            mut rebuild,
        } = state.resume
        {
            let live = &self.readings.live;
            let fix = live.location_e7.map(|_| live.utc_ms);
            let done = if medium != self.ride.shown() {
                // A ride that moved to another medium has its fixes there.
                rebuild.settle(&mut state.homing, fix.flatten());
                true
            } else {
                let upper = self.ride.recorder(medium).next_slot();
                let budget = match medium {
                    Medium::Internal => REBUILD_INTERNAL,
                    Medium::Bulk => REBUILD_BULK,
                };
                let (ride, mut slot) = (&mut self.ride, Slot::default());
                let read = rebuild.step(&mut state.homing, upper, budget, |index| {
                    match ride.read_slot(medium, store, bulk, index, &mut slot) {
                        Some(true) => decode(&slot),
                        _ => None,
                    }
                });
                read || match fix {
                    Some(utc_ms) if rebuild.caught_up(upper) => {
                        rebuild.settle(&mut state.homing, utc_ms);
                        true
                    }
                    _ => false,
                }
            };
            if state.homing.needs_course() {
                state.homing.set_course(state.course.points());
            }
            if !done {
                state.resume = Resume::Reading { medium, rebuild };
                return;
            }
            state.homing.end_replay();
            state.resume = Resume::None;
        }
        let event = state.homing.observe(self.readings.live.location_e7, now);
        if state.homing.needs_course() {
            state.homing.set_course(state.course.points());
        }
        let following = state.homing.following();
        let course = state.homing.following_course();
        if state.homing.take_pass() {
            self.ride.passed_start(now);
            self.display.redraw();
        }
        let (message, tone, cue) = match event {
            Some(Event::Following) if course => {
                ("FOLLOWING COURSE", Tone::Good, Some(Cue::OnTrack))
            }
            Some(Event::Following) => ("FOLLOWING TRACK", Tone::Good, Some(Cue::OnTrack)),
            Some(Event::OffTrack) => ("OFF TRACK", Tone::Warn, Some(Cue::OffTrack)),
            Some(Event::OnTrack) => ("BACK ON TRACK", Tone::Good, Some(Cue::OnTrack)),
            Some(Event::Dropped) => ("STOPPED FOLLOWING", Tone::Info, None),
            // Riding past the start on a loop is no news unless heading back.
            Some(Event::Arrived) if following => ("BACK AT START", Tone::Good, Some(Cue::AtStart)),
            Some(Event::CourseEnd) => ("END OF COURSE", Tone::Good, Some(Cue::AtStart)),
            _ => return,
        };
        let toast = crate::screens::chrome::Toast::new(message, tone);
        self.navigation.show(toast, NUDGE_MS, now);
        if let Some(cue) = cue {
            self.cue(cue, now);
        }
        self.display.redraw();
    }

    /// The tap on the page's pad: follow the course or the breadcrumb back,
    /// or stop.
    pub(super) fn toggle_follow(&mut self, now: u64) {
        let Some(state) = self.back.as_deref_mut() else {
            self.navigation.toast("NO TRACK", Tone::Info, now);
            return;
        };
        let follow = !state.homing.following();
        // Half read back, the track would lose the rest when following stops.
        let reading = matches!(state.resume, Resume::Reading { .. });
        if reading || !state.homing.set_following(follow, now) {
            self.navigation.toast("NO TRACK YET", Tone::Info, now);
        } else if follow {
            let message = if state.homing.following_course() {
                "FOLLOWING COURSE"
            } else {
                "FOLLOWING TRACK"
            };
            self.navigation.toast(message, Tone::Good, now);
            self.cue(Cue::OnTrack, now);
        }
    }

    /// A long press on MAP: stop following the course or the breadcrumb,
    /// mid-ride. The pad on BACK TO START can follow again.
    pub(super) fn stop_following_from_map(&mut self, now: u64) {
        let Some(state) = self.back.as_deref_mut() else {
            return;
        };
        if state.homing.following() {
            state.homing.set_following(false, now);
            self.navigation.toast("STOPPED FOLLOWING", Tone::Info, now);
            self.display.redraw();
        }
    }
}

/// What the back-to-start page shows: borrows only `Runtime::back`.
pub(super) fn view_of(back: &Option<&'static mut BackToStart>) -> Back {
    match back.as_deref() {
        None => Back::Unavailable,
        Some(state) => state.homing.guide().map_or(
            Back::Waiting {
                following: state.homing.following(),
                course: state.homing.following_course(),
            },
            Back::Guide,
        ),
    }
}

/// Bring the map page's track up to date with the breadcrumb.
pub(super) fn refresh_map(back: &mut Option<&'static mut BackToStart>) {
    let Some(state) = back.as_deref_mut() else {
        return;
    };
    let track = Track {
        available: true,
        crumbs: state.homing.crumbs(),
        here: state.homing.position(),
        heading: state.homing.heading(),
        home: state.homing.route_home(),
        course: state.homing.course(),
        ahead: state.homing.course_ahead(),
    };
    update_map(state.map, &mut state.map_key, &track);
}

/// What the map page draws: borrows `Runtime::back`'s frame.
pub(super) fn map_of<'a>(back: &'a Option<&'static mut BackToStart>) -> Option<&'a MapFrame> {
    back.as_deref().map(|state| &*state.map)
}
