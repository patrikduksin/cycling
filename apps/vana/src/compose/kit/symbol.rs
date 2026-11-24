//! One symbol in one tone, centred in its box.
use super::specs;
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::theme::Tone;

pub struct SymbolTile;

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub symbol: Symbol,
    pub tone: Tone,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.name
    }
}

specs! {
    /// A symbol. Ids: home and sheet 0–63, pause, end and done 64–127,
    /// sensors 128–191.
    pub enum Mark: Spec {
        Satellite = 0 => Spec {
            name: "SATELLITE",
            symbol: Symbol::Satellite,
            tone: Tone::Good,
        },
    }
}

impl Tile for SymbolTile {
    type Arg = Mark;
    const NAME: &'static str = "symbol";

    fn forms(mark: Mark, at: &Scaled) -> Forms {
        let ink = at.art.symbol(mark.spec().symbol).ink(0, 0);
        forms(&[Form {
            width: ink.width() as u16,
            height: ink.height() as u16,
            shapes: 1,
            runs: 0,
        }])
    }

    fn prepare(mark: Mark, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let spec = mark.spec();
        let icon = at.art.symbol(spec.symbol);
        let ink = icon.ink(0, 0);
        let rect = placed.rect.rect();
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: (rect.left + rect.right - ink.width()) / 2 - ink.left,
            top: (rect.top + rect.bottom - ink.height()) / 2 - ink.top,
            color: data.theme().tone(spec.tone),
        });
    }
}
