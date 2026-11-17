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
    pub ride: ride::Frame,
    pub main: ride::MainRegions,
}

impl Layout {
    pub const fn of(scaled: scaled::Scaled) -> Self {
        Self {
            scaled,
            chrome: chrome::regions(&scaled),
            ride: ride::frame(&scaled),
            main: ride::main_regions(&scaled),
        }
    }
}

/// The layout of the 240 × 320 canvas.
pub static BASE: Layout = Layout::of(scaled::BASE);
