//! Ride done's picture: the box the screen's painter fills with the route
//! or the power bars (`screens::done`), or, for a ride with neither, the
//! distance as one big number with `km` after it.
use super::{px, specs};
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Align;
use crate::screens::done::{Picture, Stat};
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;
use crate::screens::workout::Text;

pub struct PictureTile;

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    /// The fewest columns and rows it draws in, in units.
    pub least: (u8, u8),
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A picture. Ids: done 64–127.
    pub enum Frame: Spec {
        Done = 64 => Spec { name: "DONE", least: (120, 80) },
    }
}

impl Tile for PictureTile {
    type Arg = Frame;
    const NAME: &'static str = "picture";

    fn reads(_: Frame) -> Option<Reads> {
        Some(Reads::Metric(Metric::Distance))
    }

    fn forms(frame: Frame, at: &Scaled) -> Forms {
        let (columns, rows) = frame.spec().least;
        forms(&[Form {
            width: px(at, columns) as u16,
            height: px(at, rows) as u16,
            shapes: 0,
            runs: 2,
        }])
    }

    fn heavy() -> bool {
        true
    }

    fn prepare(_: Frame, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if data.done().map(|done| done.picture()) != Some(Picture::Distance) {
            return;
        }
        let theme = data.theme();
        let rect = placed.rect.rect();
        let faces = [at.face(LAYOUT.face.hero), at.face(LAYOUT.face.digits)];
        let unit = at.face(LAYOUT.face.heading);
        let mut number = Text::<8>::new();
        let written = Stat::Distance.write(data.metric(Metric::Distance), &mut number);
        let left = rect.left + px(at, LAYOUT.space.gutter);
        let gap = px(at, LAYOUT.space.gap);
        let room = rect
            .right
            .saturating_sub(left + gap + unit.fit_width(written.as_bytes()));
        let base = (rect.top + rect.bottom + faces[0].cap as usize) / 2;
        let run = out.fit(
            &faces,
            number.as_bytes(),
            room,
            left,
            base,
            Align::Left,
            theme.text,
        );
        let after = run.columns().end + gap;
        out.fit(
            &[unit],
            written.as_bytes(),
            rect.right.saturating_sub(after),
            after,
            base,
            Align::Left,
            theme.sub,
        );
    }
}
