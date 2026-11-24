//! The main ride page: its tree is the built-in `RideMain`
//! (`compose::builtin`), solved in the runtime's composition. Here are the
//! flags it shows under and the boxes other pages read from it.
use crate::compose::builtin::{Laid, ScreenId};
use crate::compose::node::Flags;
use crate::compose::registry::{CHIP, STATUS};
use crate::screens::Device;
use crate::screens::chrome::Rect;
use crate::screens::workout::View;

pub use crate::compose::builtin::GLIDES;

/// What the page shows that changes its tree.
pub const fn flags(course: bool, radar: bool, workout: bool, arrived: bool) -> Flags {
    let mut flags = Flags::NONE;
    if course {
        flags = flags.with(Flags::COURSE);
    }
    if radar {
        flags = flags.with(Flags::RADAR);
    }
    if workout {
        flags = flags.with(Flags::WORKOUT);
    }
    if arrived {
        flags = flags.with(Flags::ARRIVED);
    }
    flags
}

/// The flags `view` shows the page under.
pub fn flags_of(view: &View) -> Flags {
    let workout = view
        .training
        .as_ref()
        .is_some_and(|training| !training.done);
    let arrived = view.course && view.nav == crate::ride::course::nav::Sign::Arrived;
    flags(view.course, view.radar_paired, workout, arrived)
}

/// The main page on `device` for `flags`.
pub fn laid(device: &Device, flags: Flags) -> Laid {
    Laid::new(ScreenId::RideMain, device, flags, None).expect("the main page has a tree")
}

pub fn status_bar(main: &Laid) -> Option<Rect> {
    main.box_of(STATUS)
}

pub fn chip(main: &Laid) -> Option<Rect> {
    main.box_of(CHIP)
}

#[cfg(test)]
mod tests;
