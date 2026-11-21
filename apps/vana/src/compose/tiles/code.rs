//! The phone's QR code room: the light panel phone cameras want, with the
//! code's modules painted over it by `screens::phone::paint`. A tap on it
//! switches codes, or tries again once a session ended.
use crate::compose::data::{Data, PageData};
use crate::compose::input::{Action, OnPage};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::phone::{CODE_UNITS, RADIUS, paper};
use crate::screens::scaled::Scaled;

pub struct Code;

impl Tile for Code {
    type Arg = ();
    const NAME: &'static str = "code";

    fn forms((): (), at: &Scaled) -> Forms {
        let side = at.unit(CODE_UNITS) as u16;
        forms(&[Form {
            width: side,
            height: side,
            shapes: 1,
            runs: 0,
        }])
    }

    fn prepare((): (), _: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if let PageData::Phone {
            qr: Some(code),
            light,
            ..
        } = data.page()
        {
            out.shape(Shape::Bar {
                rect: code.panel(),
                radius: at.unit(RADIUS),
                color: paper(light),
            });
        }
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Page(OnPage::PhoneCode))
    }

    fn heavy() -> bool {
        true
    }
}
