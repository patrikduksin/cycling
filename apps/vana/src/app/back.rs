//! Back to start: the ride's breadcrumb, fed from fresh fixes, and nudges
//! when following it back.
//!
//! Composition gives `BackToStart` a block of its own with room for
//! `BACK_CRUMBS` points (PSRAM on the device), and hands it to
//! `Runtime::attach_back`; `Runtime` keeps only the reference, so the
//! breadcrumb costs main's stack nothing. Without it the back-to-start page
//! says NO TRACK.
use super::Runtime;
use super::cues::Cue;
use crate::ride::homing::{Event, Homing};
use crate::screens::chrome::Tone;
pub use crate::screens::ride::MapFrame;
use crate::screens::ride::{Back, MapKey, Track, update_map};

pub use crate::ride::homing::Crumb;

/// Points the breadcrumb keeps: 48 KiB, thinned to fit a ride of any length.
pub const BACK_CRUMBS: usize = 4096;
/// The nudges stay a little longer than a confirmation.
const NUDGE_MS: u64 = 4_000;

/// The breadcrumb and whether the ride it belongs to is still open.
pub struct BackToStart {
    homing: Homing<'static>,
    riding: bool,
    /// The map page's track, which `map_of` keeps up to date.
    map: &'static mut MapFrame,
    map_key: Option<MapKey>,
}

impl BackToStart {
    /// The breadcrumb in `crumbs`, which may be zeroed memory.
    pub fn new(crumbs: &'static mut [Crumb], map: &'static mut MapFrame) -> Self {
        Self {
            homing: Homing::new(crumbs),
            riding: false,
            map,
            map_key: None,
        }
    }
}

impl Runtime {
    /// Give back-to-start its breadcrumb; without one it never runs.
    pub fn attach_back(&mut self, back: &'static mut BackToStart) {
        self.back = Some(back);
    }

    /// Feed this pass's fix to the open ride's breadcrumb, and nudge on
    /// what following it back changed. A new ride starts a new breadcrumb.
    pub(super) fn track_back(&mut self, now: u64) {
        let Some(state) = self.back.as_deref_mut() else {
            return;
        };
        let riding = self.ride.active().riding();
        if riding && !state.riding {
            state.homing.reset();
        }
        state.riding = riding;
        if !riding {
            return;
        }
        let event = state.homing.observe(self.readings.live.location_e7, now);
        let following = state.homing.following();
        if state.homing.take_pass() {
            self.ride.passed_start(now);
            self.display.redraw();
        }
        let (message, tone, cue) = match event {
            Some(Event::Following) => ("FOLLOWING TRACK", Tone::Good, Some(Cue::OnTrack)),
            Some(Event::OffTrack) => ("OFF TRACK", Tone::Warn, Some(Cue::OffTrack)),
            Some(Event::OnTrack) => ("BACK ON TRACK", Tone::Good, Some(Cue::OnTrack)),
            Some(Event::Dropped) => ("STOPPED FOLLOWING", Tone::Info, None),
            // Riding past the start on a loop is no news unless heading back.
            Some(Event::Arrived) if following => ("BACK AT START", Tone::Good, Some(Cue::AtStart)),
            _ => return,
        };
        let toast = crate::screens::chrome::Toast::new(message, tone);
        self.navigation.show(toast, NUDGE_MS, now);
        if let Some(cue) = cue {
            self.cue(cue, now);
        }
        self.display.redraw();
    }

    /// The tap on the page's pad: follow the breadcrumb back, or stop.
    pub(super) fn toggle_follow(&mut self, now: u64) {
        let Some(state) = self.back.as_deref_mut() else {
            self.navigation.toast("NO TRACK", Tone::Info, now);
            return;
        };
        let follow = !state.homing.following();
        if !state.homing.set_following(follow, now) {
            self.navigation.toast("NO TRACK YET", Tone::Info, now);
        } else if follow {
            self.navigation.toast("FOLLOWING TRACK", Tone::Good, now);
            self.cue(Cue::OnTrack, now);
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
    };
    update_map(state.map, &mut state.map_key, &track);
}

/// What the map page draws: borrows `Runtime::back`'s frame.
pub(super) fn map_of<'a>(back: &'a Option<&'static mut BackToStart>) -> Option<&'a MapFrame> {
    back.as_deref().map(|state| &*state.map)
}
