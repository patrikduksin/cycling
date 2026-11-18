//! An upload empties the course while it streams in, so one that fails or is
//! cut off reads the stored course back.
use super::Runtime;
use super::cues::Cue;
use crate::kept;
use crate::kept::course::{Kept, Seen};
use crate::ride::breadcrumb::rebuild::Rebuild;
use crate::ride::breadcrumb::{Breadcrumb, Fix};
use crate::ride::course::follow::{Follower, Input};
use crate::ride::course::nav::{Event, Navigation, Sign};
use crate::ride::course::route_leg::{self, RouteLeg};
use crate::ride::course::stored::{self, Finder, Finding};
use crate::ride::course::turns::Turn;
use crate::ride::course::{self, Course};
use crate::ride::current::{Outing, Ride};
use crate::ride::journal::Read;
use crate::ride::log::{Slot, Status, decode};
use crate::ride::medium::Medium;
use crate::ride::turn_back::Leg;
use crate::screens::chrome::{Toast, Tone};
pub use crate::screens::ride::MapFrame;
use crate::screens::ride::{Fetched, FilledMap, MapCache, MapRegions, MapZoom, Track};
use device_api::bulk::ReadWrite;

pub use crate::ride::breadcrumb::Crumb;

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

/// Runs of one piece of the map's work and their time, in microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timed {
    pub runs: u32,
    pub busy_us: u32,
    pub max_us: u32,
}

impl Timed {
    fn add(&mut self, us: u64) {
        let us = u32::try_from(us).unwrap_or(u32::MAX);
        self.runs = self.runs.saturating_add(1);
        self.busy_us = self.busy_us.saturating_add(us);
        self.max_us = self.max_us.max(us);
    }
}

/// The map's fills (projecting the course and the roads) and road
/// extractions, timed by the composition's microsecond clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapWork {
    pub fill: Timed,
    pub extract: Timed,
    /// Extracted tiles whose minor roads did not all fit.
    pub cuts: u32,
}

/// The breadcrumb, the course it follows and the open ride they belong to.
pub struct RideTrack {
    breadcrumb: Breadcrumb<'static>,
    follower: Follower<'static>,
    route_leg: RouteLeg,
    outing: Option<Outing>,
    map: MapCache<'static>,
    zoom: MapZoom,
    /// The loaded course and whether it matches the stored one.
    pub(super) course: &'static mut Course,
    /// Finds the turns of a course while it uploads.
    finder: &'static mut Finder,
    pub(super) kept: Kept,
    resume: Resume,
    pub(super) work: MapWork,
    pub(super) since_attached: MapWork,
}

impl RideTrack {
    /// The breadcrumb in `crumbs`, the course and room for it in the
    /// ride's frame in `line`; all may be zeroed memory.
    pub fn new(
        crumbs: &'static mut [Crumb],
        line: &'static mut [Crumb],
        turns: &'static mut [Turn],
        map: &'static mut MapFrame,
        course: &'static mut Course,
        finder: &'static mut Finder,
    ) -> Self {
        Self {
            breadcrumb: Breadcrumb::new(crumbs),
            follower: Follower::new(line, turns),
            route_leg: RouteLeg::default(),
            outing: None,
            map: MapCache::new(map),
            zoom: MapZoom::default(),
            course,
            finder,
            kept: Kept::default(),
            resume: Resume::None,
            work: MapWork::default(),
            since_attached: MapWork::default(),
        }
    }

    pub(super) fn fetch_roads(
        &mut self,
        engine: &mut terrain::Lookahead,
        clock: Option<fn() -> u64>,
    ) {
        let Some(frame) = self.breadcrumb.frame() else {
            return;
        };
        let started = clock.map(|clock| clock());
        let fetched = self.map.fetch_roads(engine, frame);
        if fetched != Fetched::Nothing {
            self.timed(started, clock, |work| &mut work.extract);
        }
        if fetched == Fetched::Cut {
            for work in [&mut self.work, &mut self.since_attached] {
                work.cuts = work.cuts.saturating_add(1);
            }
        }
    }

    fn timed(
        &mut self,
        started: Option<u64>,
        clock: Option<fn() -> u64>,
        which: fn(&mut MapWork) -> &mut Timed,
    ) {
        if let (Some(started), Some(clock)) = (started, clock) {
            let us = clock().saturating_sub(started);
            which(&mut self.work).add(us);
            which(&mut self.since_attached).add(us);
        }
    }
}

impl Runtime {
    /// Read the stored course when it is unread, unless an upload is
    /// replacing it.
    pub(super) fn service_course(&mut self, media: &mut impl ReadWrite, now: u64) {
        let seen = Seen {
            ready: self.ride.bulk().status().usable(),
            uploading: self.phone_uploading(),
        };
        let Some(track) = self.track.as_deref_mut() else {
            return;
        };
        if !track
            .kept
            .service(seen, media, &mut *track.course, now)
            .read
        {
            return;
        }
        course_changed(track, media);
        self.display.redraw();
    }

    /// The loaded course, while one is.
    pub fn course(&self) -> Option<&Course> {
        self.track
            .as_deref()
            .map(|track| &*track.course)
            .filter(|course| !course.is_empty())
    }

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
        self.follow_course(store, bulk, now);
        if let Some(state) = self.track.as_deref_mut() {
            step_route_leg(state, self.readings.live.speed_mm_s, now);
        }
    }

    fn follow_course(
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
            state.breadcrumb.reset();
            state.follower.handle(Input::NewRide);
            state.zoom = MapZoom::default();
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
                rebuild.settle(&mut state.breadcrumb, fix.flatten());
                true
            } else {
                let upper = self.ride.recorder(medium).next_slot();
                let budget = match medium {
                    Medium::Internal => REBUILD_INTERNAL,
                    Medium::Bulk => REBUILD_BULK,
                };
                let (ride, mut slot) = (&mut self.ride, Slot::default());
                let read = rebuild.step(&mut state.breadcrumb, upper, budget, |index| {
                    match ride.read_slot(medium, store, bulk, index, &mut slot) {
                        Read::Slot => decode(&slot),
                        Read::Invalid | Read::Failed => None,
                    }
                });
                read || match fix {
                    Some(utc_ms) if rebuild.caught_up(upper) => {
                        rebuild.settle(&mut state.breadcrumb, utc_ms);
                        true
                    }
                    _ => false,
                }
            };
            place_course(state, bulk);
            if !done {
                state.resume = Resume::Reading { medium, rebuild };
                return;
            }
            state.breadcrumb.end_replay();
            state.resume = Resume::None;
        }
        let fix = state.breadcrumb.observe(self.readings.reported, now);
        let speed = self.readings.live.speed_mm_s;
        let paused = self.ride.active().status() == Status::Paused;
        if let (Some(Fix { moved: true, .. }), Some(speed), false) = (fix, speed, paused) {
            state.zoom.observe_riding_at(speed);
        }
        let event = state.follower.handle(Input::Observe {
            now,
            fix,
            speed_mm_s: speed,
            paused,
        });
        place_course(state, bulk);
        if state.breadcrumb.take_pass() {
            let playing = self.training.player.is_some();
            if let Some(lap) =
                self.ride
                    .passed_start(self.preferences.options.auto_lap, playing, now)
            {
                self.notices.lap_closed(lap, now);
            }
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
            Event::Arrived => Cue::Arrived,
        };
        self.cue(cue, now);
        self.display.redraw();
    }

    pub(super) fn following(&self) -> bool {
        self.track
            .as_deref()
            .is_some_and(|state| state.follower.following())
    }

    pub(super) fn stop_following_from_map(&mut self, now: u64) {
        if let Some(state) = self.track.as_deref_mut()
            && state.follower.following()
        {
            state.follower.handle(Input::LetGo);
            self.navigation.toast("COURSE OFF", Tone::Info, now);
            self.display.redraw();
        }
    }
}

/// The way home to warn about: none while a recovered ride's track is
/// still being read, as its length so far is not the way home.
pub(super) fn way_home(track: &RideTrack, speed_mm_s: Option<u32>, moving_ms: u64) -> Option<Leg> {
    if matches!(track.resume, Resume::Reading { .. }) {
        return None;
    }
    Leg::of(
        &track.breadcrumb.guide()?,
        track.breadcrumb.way_back(),
        speed_mm_s,
        moving_ms,
    )
}

/// Follow the course as it is now, or nothing once it is cleared. Once
/// it is placed, its turns are those stored beside it on `media`, or
/// found by the scan when none of its own are there.
pub(super) fn course_changed(track: &mut RideTrack, media: &mut impl ReadWrite) {
    let course = &*track.course;
    let mut read =
        |turns: &mut [Turn]| match kept::course::load_turns(media, course::key(course), turns) {
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
        };
    track.follower.handle(Input::Course {
        points: course.points(),
        frame: track.breadcrumb.frame(),
        read: &mut read,
    });
}

fn place_course(track: &mut RideTrack, media: &mut impl ReadWrite) {
    if track.follower.unplaced() && track.breadcrumb.frame().is_some() {
        course_changed(track, media);
    }
}

/// Store the turns `found` while the course uploaded beside it, or,
/// with none to trust, remove what is there.
pub(super) fn save_turns(
    track: &mut RideTrack,
    media: &mut impl ReadWrite,
    found: Option<stored::Found>,
) {
    let key = course::key(track.course);
    let result = match found {
        Some(found) => {
            log::info!(
                target: "phone",
                "VANA_PHONE course_turns count={} full={} reach={}",
                found.count,
                found.full,
                found.reach
            );
            let turns = &track.follower.upload_room()[..found.count];
            kept::course::save_turns(media, key, turns, found.full)
        }
        None => kept::course::delete_turns(media),
    };
    if let Err(error) = result {
        log::warn!(target: "phone", "VANA_PHONE course_turns_not_saved error={error:?}");
    }
}

/// The course to build from an upload, and what finds its turns into
/// the turn list meanwhile.
pub(super) fn upload(track: &mut RideTrack) -> (&mut Course, Finding<'_>) {
    let finding = Finding {
        finder: track.finder,
        turns: track.follower.upload_room(),
    };
    (track.course, finding)
}

pub(super) fn refresh_map(
    track: &mut Option<&'static mut RideTrack>,
    now: u64,
    speed_mm_s: Option<u32>,
    radar: bool,
    map: &'static MapRegions,
    light: bool,
    clock: Option<fn() -> u64>,
) {
    let Some(state) = track.as_deref_mut() else {
        return;
    };
    let drawn = Track {
        crumbs: state.breadcrumb.crumbs(),
        here: state.breadcrumb.position(),
        here_taken_ms: state.breadcrumb.position_taken_ms(),
        heading: state.breadcrumb.heading(),
        speed_mm_s,
        course: state.follower.course(),
        placement: state.follower.placement(),
        ahead: state.follower.ahead(),
        joinable: state.follower.joinable(),
        leg: state.route_leg.end(),
        guidance: state.follower.shown(now).guidance,
        left_m: state.follower.to_end_m(),
        reach: state.zoom.reach(),
        radar,
        light,
    };
    let started = clock.map(|clock| clock());
    if state.map.update(&drawn, map) {
        state.timed(started, clock, |work| &mut work.fill);
    }
}

fn step_route_leg(state: &mut RideTrack, speed_mm_s: Option<u32>, now: u64) {
    let course = state.follower.course();
    let junction = state.follower.shown(now).guidance.and_then(|g| g.junction);
    let split = route_leg::split(course, state.follower.ahead(), junction);
    state.route_leg.observe(&route_leg::Input {
        course,
        placement: state.follower.placement(),
        here: state.breadcrumb.position(),
        here_taken_ms: state.breadcrumb.position_taken_ms(),
        heading: state.breadcrumb.heading(),
        speed_mm_s,
        split: split.map(|(end, _)| end),
    });
}

pub(super) fn still_due(track: &Option<&'static mut RideTrack>, cue: Navigation, now: u64) -> bool {
    track
        .as_deref()
        .is_some_and(|state| state.follower.still_due(cue, now))
}

pub(super) fn sign_of(track: &Option<&'static mut RideTrack>, now: u64) -> Sign {
    track
        .as_deref()
        .map_or(Sign::None, |state| state.follower.shown(now).sign)
}

pub(super) fn course_placed(track: &Option<&'static mut RideTrack>) -> bool {
    track
        .as_deref()
        .is_some_and(|state| !state.follower.course().is_empty())
}

pub(super) fn map_of<'a>(track: &'a Option<&'static mut RideTrack>) -> Option<FilledMap<'a>> {
    track.as_deref()?.map.filled()
}
