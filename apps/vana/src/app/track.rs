use super::Runtime;
use super::cues::Cue;
use super::recording::{Medium, Outing, Ride};
use crate::ride::course::follow::Follower;
use crate::ride::course::nav::{Event, Sign};
use crate::ride::course::stored::{self, Finder, Finding};
use crate::ride::course::{self, Course};
use crate::ride::homing::Homing;
use crate::ride::homing::rebuild::Rebuild;
use crate::ride::log::{Slot, Status, decode};
use crate::ride::turn_back::Leg;
use crate::screens::chrome::{Toast, Tone};
pub use crate::screens::ride::MapFrame;
use crate::screens::ride::{FilledMap, MapCache, Track};
use device_api::bulk::ReadWrite;

pub use crate::ride::homing::Crumb;

/// Points the breadcrumb keeps: 48 KiB, thinned to fit a ride of any length.
pub const TRACK_CRUMBS: usize = 4096;
const NUDGE_MS: u64 = 2 * super::presentation::TOAST_MS;
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

/// The breadcrumb, the course it follows and the open ride they belong to.
pub struct RideTrack {
    homing: Homing<'static>,
    follower: Follower<'static>,
    outing: Option<Outing>,
    map: MapCache<'static>,
    /// The loaded course and whether it matches the stored one.
    pub(super) course: &'static mut Course,
    /// Finds the turns of a course while it uploads.
    finder: &'static mut Finder,
    pub(super) kept: super::course::Kept,
    resume: Resume,
}

impl RideTrack {
    /// The breadcrumb in `crumbs`, the course and room for it in the
    /// ride's frame in `line`; all may be zeroed memory.
    pub fn new(
        crumbs: &'static mut [Crumb],
        line: &'static mut [Crumb],
        turns: &'static mut [crate::ride::course::turns::Turn],
        map: &'static mut MapFrame,
        course: &'static mut Course,
        finder: &'static mut Finder,
    ) -> Self {
        Self {
            homing: Homing::new(crumbs),
            follower: Follower::new(line, turns),
            outing: None,
            map: MapCache::new(map),
            course,
            finder,
            kept: super::course::Kept::default(),
            resume: Resume::None,
        }
    }

    /// The way home to warn about: none while a recovered ride's track is
    /// still being read, as its length so far is not the way home.
    pub(super) fn leg(&self, speed_mm_s: Option<u32>, moving_ms: u64) -> Option<Leg> {
        if matches!(self.resume, Resume::Reading { .. }) {
            return None;
        }
        Leg::of(
            &self.homing.guide()?,
            self.homing.way_back(),
            speed_mm_s,
            moving_ms,
        )
    }

    /// Follow the course as it is now, or nothing once it is cleared. Once
    /// it is placed, its turns are those stored beside it on `media`, or
    /// found by the scan when none of its own are there.
    pub(super) fn course_changed(&mut self, media: &mut impl ReadWrite) {
        let course = &*self.course;
        self.follower
            .set_course(course.points(), self.homing.frame());
        self.follower.take_turns(course.points(), |turns| {
            match stored::load(media, course::key(course), turns) {
                Ok(Some(stored)) => {
                    log::info!(
                        target: "sdk.course",
                        "turns_stored count={} full={}",
                        stored.count,
                        stored.full
                    );
                    Some((stored.count, stored.full))
                }
                Ok(None) => {
                    log::info!(target: "sdk.course", "turns_scanned");
                    None
                }
                Err(error) => {
                    log::warn!(target: "sdk.course", "turns_not_read error={error:?}");
                    None
                }
            }
        });
    }

    fn place_course(&mut self, media: &mut impl ReadWrite) {
        if self.follower.unplaced() && self.homing.frame().is_some() {
            self.course_changed(media);
        }
    }

    /// Store the turns `found` while the course uploaded beside it, or,
    /// with none to trust, remove what is there.
    pub(super) fn save_turns(&mut self, media: &mut impl ReadWrite, found: Option<stored::Found>) {
        let key = course::key(self.course);
        let result = match found {
            Some(found) => {
                log::info!(
                    target: "phone",
                    "VANA_PHONE course_turns count={} full={} reach={}",
                    found.count,
                    found.full,
                    found.reach
                );
                let turns = &self.follower.turn_room()[..found.count];
                stored::save(media, key, turns, found.full)
            }
            None => stored::delete(media),
        };
        if let Err(error) = result {
            log::warn!(target: "phone", "VANA_PHONE course_turns_not_saved error={error:?}");
        }
    }

    /// The course to build from an upload, and what finds its turns into
    /// the turn list meanwhile.
    pub(super) fn upload(&mut self) -> (&mut Course, Finding<'_>) {
        let finding = Finding {
            finder: self.finder,
            turns: self.follower.turn_room(),
        };
        (self.course, finding)
    }
}

impl Runtime {
    /// Without a track the map says NO TRACK and no course loads.
    pub fn attach_track(&mut self, track: &'static mut RideTrack) {
        self.track = Some(track);
    }

    pub(super) fn observe_track(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        bulk: &mut impl device_api::bulk::ReadWrite,
        now: u64,
    ) {
        let Some(state) = self.track.as_deref_mut() else {
            return;
        };
        let outing = self.ride.outing();
        let riding = outing.is_some();
        if riding && outing != state.outing {
            state.homing.reset();
            state.follower.reset();
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
        state.outing = outing;
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
            state.place_course(bulk);
            if !done {
                state.resume = Resume::Reading { medium, rebuild };
                return;
            }
            state.homing.end_replay();
            state.resume = Resume::None;
        }
        let fix = state.homing.observe(self.readings.reported, now);
        let event = state
            .follower
            .observe(fix, now, self.readings.live.speed_mm_s);
        state.place_course(bulk);
        if state.homing.take_pass() {
            self.ride.passed_start(now);
            self.display.redraw();
        }
        let Some(event) = event else {
            return;
        };
        if event == Event::Following {
            let toast = Toast::new("FOLLOWING COURSE", Tone::Good);
            self.navigation.show(toast, NUDGE_MS, now);
        }
        let cue = match event {
            Event::Following | Event::OnCourse => Cue::OnCourse,
            Event::TurnSoon => Cue::TurnSoon,
            Event::TurnNow => Cue::TurnNow,
            Event::OffCourse | Event::StillOff => Cue::OffCourse,
            Event::Arrived => Cue::Start,
        };
        self.cue(cue, now);
        self.display.redraw();
    }

    pub(super) fn stop_following_from_map(&mut self) {
        if let Some(state) = self.track.as_deref_mut()
            && state.follower.following()
        {
            state.follower.stop();
            self.display.redraw();
        }
    }
}

pub(super) fn refresh_map(track: &mut Option<&'static mut RideTrack>, now: u64) {
    let Some(state) = track.as_deref_mut() else {
        return;
    };
    let drawn = Track {
        available: true,
        crumbs: state.homing.crumbs(),
        here: state.homing.position(),
        heading: state.homing.heading(),
        course: state.follower.course(),
        ahead: state.follower.ahead(),
        guidance: state.follower.guidance(now),
        left_m: state.follower.to_end_m(),
        badge: None,
    };
    state.map.update(&drawn);
}

pub(super) fn sign_of(track: &Option<&'static mut RideTrack>, now: u64) -> Sign {
    track
        .as_deref()
        .map_or(Sign::None, |state| state.follower.sign(now))
}

pub(super) fn map_of<'a>(track: &'a Option<&'static mut RideTrack>) -> Option<FilledMap<'a>> {
    track.as_deref()?.map.filled()
}
