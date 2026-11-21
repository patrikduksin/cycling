use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::WHITE;
use crate::screens::icons::{Icon as Art, StaticIcon};
use crate::screens::scaled::Scaled;

pub struct Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Refresh,
    Plus,
    Arrow,
    Puck,
}

impl Glyph {
    fn art(self, at: &Scaled) -> &'static Art {
        match self {
            Self::Refresh => at.art.refresh,
            Self::Plus => at.art.plus,
            Self::Arrow => at.art.arrow,
            Self::Puck => at.art.puck,
        }
    }
}

impl Arg for Glyph {
    fn decode(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::Refresh,
            1 => Self::Plus,
            2 => Self::Arrow,
            3 => Self::Puck,
            _ => return None,
        })
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Refresh => "REFRESH",
            Self::Plus => "PLUS",
            Self::Arrow => "ARROW",
            Self::Puck => "PUCK",
        }
    }
}

impl Tile for Icon {
    type Arg = Glyph;
    const NAME: &'static str = "icon";

    fn forms(glyph: Glyph, at: &Scaled) -> Forms {
        let size = u16::from(glyph.art(at).size);
        forms(&[Form {
            width: size,
            height: size,
            shapes: 1,
            runs: 0,
        }])
    }

    fn prepare(glyph: Glyph, placed: &Placed, at: &Scaled, _: &dyn Data, out: &mut Canvas) {
        let art = glyph.art(at);
        let rect = placed.rect;
        out.shape(Shape::Icon {
            icon: StaticIcon(art),
            left: usize::from(rect.left),
            top: usize::from(rect.bottom) - usize::from(art.size),
            color: WHITE,
        });
    }
}
