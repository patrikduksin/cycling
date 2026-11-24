use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Align;
use crate::screens::scaled::Scaled;
use crate::screens::sensors::widest;
use crate::screens::theme::LAYOUT;
use crate::screens::workout::Text;
use core::fmt::Write;

pub struct SensorSearch;

const LINES: [&[u8]; 2] = [b"NO SENSORS FOUND", b"TAP HERE TO SEARCH"];
const WAKE: &[u8] = b"WAKE A SENSOR";

impl Tile for SensorSearch {
    type Arg = ();
    const NAME: &'static str = "sensor search";

    fn forms((): (), at: &Scaled) -> Forms {
        let space = &LAYOUT.space;
        let tag = at.face(LAYOUT.face.tag);
        let line = widest::message(tag).max(widest::of(tag, LINES));
        let added = (widest::digits(tag, 2) + tag.fit_width(b" ADDED")).max(tag.fit_width(WAKE));
        let width = 2 * px(at, space.gutter) + px(at, space.dot) + px(at, space.gap) + line;
        let searching = tag.fit_width(b"SEARCHING");
        let beside = 2 * px(at, space.gutter)
            + px(at, space.dot)
            + 2 * px(at, space.gap)
            + searching
            + added;
        forms(&[Form {
            width: width.max(beside) as u16,
            height: (tag.fit_height() + px(at, space.pad)) as u16,
            shapes: 1,
            runs: 2,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let theme = data.theme();
        let space = &LAYOUT.space;
        let rect = placed.rect.rect();
        let tag = at.face(LAYOUT.face.tag);
        let base = rect.top + tag.fit_height();
        let left = rect.left + px(at, space.gutter);
        let mut right = rect.right - px(at, space.gutter);
        let search = menu.search();
        let color = theme.tone(search.tone);
        let dot = px(at, space.dot);
        out.shape(Shape::Disc {
            cx: (4 * left + 2 * dot) as i32,
            cy: (4 * base - 2 * usize::from(tag.cap)) as i32,
            radius: (2 * dot) as i32,
            color,
        });
        let mut added = Text::<12>::new();
        let aside = if search.added > 0 {
            let _ = write!(added, "{} ADDED", search.added);
            Some((added.as_bytes(), theme.good))
        } else {
            search.wake.then_some((WAKE, theme.sub))
        };
        if let Some((words, color)) = aside {
            let run = out.fit(
                &[tag],
                words,
                right - left,
                right,
                base,
                Align::Right,
                color,
            );
            right = run.left.saturating_sub(px(at, space.gap));
        }
        let text = left + dot + px(at, space.gap);
        out.fit(
            &[tag],
            search.text.as_bytes(),
            right.saturating_sub(text),
            text,
            base,
            Align::Left,
            color,
        );
    }
}
