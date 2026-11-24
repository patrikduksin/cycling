//! A road's profile: a three-column summary over the climb chart, which
//! `screens::ride::climb` paints row by row under it.
use super::px;
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::ride::climb::{Stretch, Sums};
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;
use crate::screens::workout::Text;

pub struct Profile;

const ROOM: [&[u8]; 3] = [b"0.75 km", b"9.9%", b"12%"];

fn summary_rows(at: &Scaled) -> usize {
    let space = &LAYOUT.space;
    px(at, space.pad)
        + at.face(LAYOUT.face.tag).fit_height()
        + px(at, space.tight)
        + at.face(LAYOUT.face.summary).fit_height()
        + px(at, space.gap)
}

pub fn chart(profile: Rect, at: &Scaled) -> Rect {
    let gutter = px(at, LAYOUT.space.gutter);
    Rect {
        left: profile.left + gutter,
        right: profile
            .right
            .saturating_sub(gutter)
            .max(profile.left + gutter),
        top: (profile.top + summary_rows(at)).min(profile.bottom),
        bottom: profile.bottom,
    }
}

impl Tile for Profile {
    type Arg = ();
    const NAME: &'static str = "road profile";

    fn forms((): (), at: &Scaled) -> Forms {
        let space = &LAYOUT.space;
        let numbers: usize = ROOM
            .iter()
            .map(|text| at.face(LAYOUT.face.summary).fit_width(text))
            .sum();
        forms(&[Form {
            width: (2 * px(at, space.gutter) + numbers + 2 * px(at, space.gap)) as u16,
            height: (summary_rows(at) + px(at, space.chart)) as u16,
            shapes: 2,
            runs: 6,
        }])
    }

    fn heavy() -> bool {
        true
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(road) = data.road() else {
            return;
        };
        let theme = data.theme();
        let space = &LAYOUT.space;
        let rect = placed.rect.rect();
        let gutter = px(at, space.gutter);
        let (left, right) = (rect.left + gutter, rect.right - gutter);
        let tag = at.face(LAYOUT.face.tag);
        let caption_base = rect.top + px(at, space.pad) + tag.fit_height();
        let number = at.face(LAYOUT.face.summary);
        let number_base = caption_base + px(at, space.tight) + number.fit_height();
        let first = match road.stretch {
            Stretch::Flat => "LAST",
            Stretch::Climb | Stretch::Course => "CLIMB",
        };
        let sums = road.frame.sums;
        let mut texts: [Text<12>; 3] = Default::default();
        match sums {
            Some(sums) => sums.write_length(&mut texts[0]),
            None => {
                let _ = texts[0].push_str("--");
            }
        }
        Sums::write_grade(sums.and_then(|sums| sums.average_tenths), &mut texts[1]);
        Sums::write_grade(sums.and_then(|sums| sums.steepest_tenths), &mut texts[2]);
        let third = (right - left) / 3;
        let columns = [
            (left, Align::Left),
            ((left + right) / 2, Align::Center),
            (right, Align::Right),
        ];
        for ((caption, text), (x, align)) in [first, "AVG", "MAX"].iter().zip(&texts).zip(columns) {
            let width = if align == Align::Center {
                third
            } else {
                right - left
            };
            out.fit(
                &[tag],
                caption.as_bytes(),
                width,
                x,
                caption_base,
                align,
                theme.sub,
            );
            out.fit(
                &[number],
                text.as_bytes(),
                width,
                x,
                number_base,
                align,
                theme.text,
            );
        }
        if let Some((cx, cy)) = road.frame.rider() {
            let ink = 2 * px(at, space.marker) as i32;
            let halo = ink + 4 * px(at, space.stroke) as i32;
            out.shape(Shape::Disc {
                cx,
                cy,
                radius: halo,
                color: theme.bg,
            });
            out.shape(Shape::Disc {
                cx,
                cy,
                radius: ink,
                color: theme.text,
            });
        }
    }
}
