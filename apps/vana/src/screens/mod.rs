pub mod bests;
pub mod brand;
pub mod canvas;
#[cfg(any(test, feature = "invariants"))]
pub mod check;
pub mod chrome;
pub mod cue;
pub mod form;
pub mod format;
pub mod history;
pub mod home;
pub mod icons;
pub mod kinetic;
pub mod limits;
pub mod look;
pub mod phone;
pub mod power;
pub mod qr;
pub mod ready;
pub mod ride;
pub mod scaled;
pub mod scroll;
pub mod sensors;
pub mod settings;
pub mod setup;
pub mod sign;
pub mod touch;
pub mod workout;
pub mod workouts;

const _: () = assert!(
    limits::HEIGHT <= firmware_shell::shell::MAX_HEIGHT,
    "the shell keys fewer bands than the tallest panel has"
);

/// Every screen that lays out from the panel, laid out once on one panel.
/// Composition keeps it in a `static`, so a screen that does not fit the
/// panel fails that device's build. A layout that depends on what a frame
/// shows, a course or a radar, is a field of its own, chosen per frame.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Layout {
    pub scaled: scaled::Scaled,
    pub chrome: chrome::Regions,
    pub sign: sign::Regions,
    pub ride: ride::Frame,
    pub main: ride::MainRegions,
    pub stats: ride::StatsRegions,
    pub lap: ride::LapRegions,
    pub map: ride::MapRegions,
    pub climb: ride::ClimbRegions,
    pub summary: ride::summary::Regions,
    pub workout: ride::WorkoutRegions,
    pub paused: ride::PausedRegions,
    pub end: ride::EndRegions,
}

impl Layout {
    pub const fn of(scaled: scaled::Scaled) -> Self {
        Self {
            scaled,
            chrome: chrome::regions(&scaled),
            sign: sign::regions(&scaled),
            ride: ride::frame(&scaled),
            main: ride::main_regions(&scaled),
            stats: ride::stats_regions(&scaled),
            lap: ride::lap_regions(&scaled),
            map: ride::map_regions(&scaled),
            climb: ride::climb_regions(&scaled),
            summary: ride::summary::regions(&scaled),
            workout: ride::workout_regions(&scaled),
            paused: ride::paused_regions(&scaled),
            end: ride::end_regions(&scaled),
        }
    }
}

/// The layout of the 240 × 320 canvas.
pub static BASE: Layout = Layout::of(scaled::BASE);

/// The simulator's test panels, for screens' own tests.
#[cfg(test)]
pub(crate) mod panels {
    use super::{Layout, scaled::Scaled};

    pub(crate) static WIDE: Layout = Layout::of(Scaled::of(240, 400));
    pub(crate) static SMALL: Layout = Layout::of(Scaled::of(180, 240));
    pub(crate) static ALL: [&Layout; 3] = [&super::BASE, &WIDE, &SMALL];

    /// The probe panel: 3/4, and taller than 3/4 of the C606.
    #[test]
    fn a_panel_taller_than_its_scale_lays_out() {
        let probe = Layout::of(Scaled::of(180, 320));
        assert_eq!(probe.map.free.drawn.height(), 280);
    }
}
