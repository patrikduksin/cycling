pub mod bests;
pub mod brand;
pub mod canvas;
#[cfg(any(test, feature = "invariants"))]
pub mod check;
pub mod chrome;
pub mod cue;
#[cfg(test)]
mod cue_screens;
pub(crate) mod data;
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

use device_api::input::{Fixture, Fixtures, Role, Side};

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
    pub corners: cue::Corners,
    pub chrome: chrome::Regions,
    pub sign: sign::Regions,
    pub ride: ride::Frame,
}

impl Layout {
    pub const fn of(scaled: scaled::Scaled, buttons: &Fixtures) -> Self {
        let corners = cue::Corners::of(buttons);
        Self {
            scaled,
            corners,
            chrome: chrome::regions(&scaled),
            sign: sign::regions(&scaled),
            ride: ride::frame(&scaled),
        }
    }
}

/// The buttons the base layout lays out for, the C606's: BACK high and
/// MOVE low on the left side, ACT low on the right.
pub const BASE_BUTTONS: Fixtures = Fixtures::new(&[
    Fixture {
        id: 0,
        role: Some(Role::Back),
        edge: Side::Left,
        at: 250,
    },
    Fixture {
        id: 1,
        role: Some(Role::Move),
        edge: Side::Left,
        at: 750,
    },
    Fixture {
        id: 2,
        role: Some(Role::Act),
        edge: Side::Right,
        at: 750,
    },
]);

/// The layout of the base 240 × 320 panel, the C606's.
pub static BASE: Layout = Layout::of(scaled::BASE, &BASE_BUTTONS);

/// The simulator's test panels, for screens' own tests.
#[cfg(test)]
pub(crate) mod panels {
    use super::{BASE_BUTTONS, Layout, scaled::Scaled};

    pub(crate) static WIDE: Layout = Layout::of(Scaled::of(240, 400), &BASE_BUTTONS);
    pub(crate) static SMALL: Layout = Layout::of(Scaled::of(180, 240), &BASE_BUTTONS);
    pub(crate) static ALL: [&Layout; 3] = [&super::BASE, &WIDE, &SMALL];

    /// The probe panel: 3/4, and taller than 3/4 of the C606.
    #[test]
    fn a_panel_taller_than_its_scale_lays_out() {
        let probe = Layout::of(Scaled::of(180, 320), &BASE_BUTTONS);
        assert_eq!(super::ride::map::ground_rect(&probe.scaled).height(), 281);
    }
}
