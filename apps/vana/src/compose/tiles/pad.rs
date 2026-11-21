//! A pad that steps the edited number down or up by its step, under a
//! finger as MOVE and ACT do.
use crate::compose::data::Data;
use crate::compose::input::{Action, OnPage};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, RAISED, WHITE};
use crate::screens::scaled::Scaled;
use firmware_shell::rendering::font::FaceRole;

pub struct Pad;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Lower,
    Raise,
}

impl Arg for Way {
    fn decode(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Lower),
            1 => Some(Self::Raise),
            _ => None,
        }
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Lower => "LOWER",
            Self::Raise => "RAISE",
        }
    }
}

const HEIGHT_UNITS: usize = 58;
const RADIUS_UNITS: usize = 12;

impl Tile for Pad {
    type Arg = Way;
    const NAME: &'static str = "pad";

    fn forms(_: Way, at: &Scaled) -> Forms {
        let label = at.face(FaceRole::Large).fit_width(b"+5");
        forms(&[Form {
            width: (label + at.unit(16)) as u16,
            height: at.unit(HEIGHT_UNITS) as u16,
            shapes: 1,
            runs: 1,
        }])
    }

    fn prepare(way: Way, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let step = data.edited().map_or(1, |edited| edited.step);
        let label: &[u8] = match (way, step) {
            (Way::Lower, 1) => b"-1",
            (Way::Raise, 1) => b"+1",
            (Way::Lower, _) => b"-5",
            (Way::Raise, _) => b"+5",
        };
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: at.unit(RADIUS_UNITS),
            color: RAISED,
        });
        let large = at.face(FaceRole::Large);
        let base = rect.top + (rect.height() + large.cap as usize) / 2;
        let (width, middle) = (rect.width(), rect.center_x());
        out.fit(&[large], label, width, middle, base, Align::Center, WHITE);
    }

    fn action(way: Way) -> Option<Action> {
        Some(Action::Page(match way {
            Way::Lower => OnPage::Lower,
            Way::Raise => OnPage::Raise,
        }))
    }
}
