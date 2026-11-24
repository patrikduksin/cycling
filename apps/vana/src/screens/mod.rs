pub mod brand;
pub mod canvas;
#[cfg(any(test, feature = "invariants"))]
pub mod check;
pub mod chrome;
pub mod cue;
pub(crate) mod data;
pub mod done;
pub mod format;
pub mod home;
pub mod icons;
pub mod kinetic;
pub mod limits;
pub mod look;
pub mod power;
pub mod ride;
pub mod scaled;
pub mod scroll;
pub mod sensors;
pub mod settings;
pub mod sheet;
pub mod sign;
pub mod theme;
pub mod touch;
pub mod workout;
pub mod workouts;

use device_api::input::Fixtures;

const _: () = assert!(
    limits::HEIGHT <= firmware_shell::shell::MAX_HEIGHT,
    "the shell keys fewer bands than the tallest panel has"
);

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Device {
    pub scaled: scaled::Scaled,
    pub keys: cue::Keys,
}

impl Device {
    /// Panics, at compile time in a const, when a role has no key or two
    /// keys' tabs would overlap.
    pub const fn of(scaled: scaled::Scaled, buttons: &Fixtures) -> Self {
        let keys = cue::Keys::of(buttons);
        keys.assert_apart(&scaled);
        Self { scaled, keys }
    }
}

/// The C606: a 240 × 320 panel at scale 1.
pub static C606: Device = Device::of(scaled::BASE, &limits::C606_BUTTONS);

/// The simulator's test panels, for screens' own tests.
#[cfg(test)]
pub(crate) mod panels {
    use super::{Device, limits::C606_BUTTONS, scaled::Scaled};

    pub(crate) static WIDE: Device = Device::of(Scaled::of(240, 400), &C606_BUTTONS);
    pub(crate) static SMALL: Device = Device::of(Scaled::of(180, 240), &C606_BUTTONS);
    pub(crate) static ALL: [&Device; 3] = [&super::C606, &WIDE, &SMALL];

    /// The probe panel: 3/4, and taller than 3/4 of the C606.
    #[test]
    fn a_panel_taller_than_its_scale_lays_out() {
        let probe = Device::of(Scaled::of(180, 320), &C606_BUTTONS);
        assert_eq!(super::ride::map::ground_rect(&probe.scaled).height(), 281);
    }
}
