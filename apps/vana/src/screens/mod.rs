pub mod bests;
pub mod brand;
pub mod canvas;
#[cfg(any(test, feature = "invariants"))]
pub mod check;
pub mod chrome;
pub mod cue;
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
pub mod theme;
pub mod touch;
pub mod workout;
pub mod workouts;

use device_api::input::{Fixture, Fixtures, Role, Side};

const _: () = assert!(
    limits::HEIGHT <= firmware_shell::shell::MAX_HEIGHT,
    "the shell keys fewer bands than the tallest panel has"
);

/// What a device's screens are laid out for: its panel, at the scale its
/// faces and icons are generated for, and the corners its buttons' cues
/// take. A screen's boxes come from its tree, solved for these.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Device {
    pub scaled: scaled::Scaled,
    pub corners: cue::Corners,
}

impl Device {
    pub const fn of(scaled: scaled::Scaled, buttons: &Fixtures) -> Self {
        Self {
            scaled,
            corners: cue::Corners::of(buttons),
        }
    }
}

/// The C606's buttons: BACK high and MOVE low on the left side, ACT low on
/// the right.
pub const C606_BUTTONS: Fixtures = Fixtures::new(&[
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

/// The C606: a 240 × 320 panel at scale 1.
pub static C606: Device = Device::of(scaled::BASE, &C606_BUTTONS);

/// The simulator's test panels, for screens' own tests.
#[cfg(test)]
pub(crate) mod panels {
    use super::{C606_BUTTONS, Device, scaled::Scaled};

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
