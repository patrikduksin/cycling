//! One symbol, centred in its box: in its tone, or white on a disc of its
//! tone, as a phone-style screen shows its state.
use super::{px, specs};
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::done::Kept;
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Tone};

pub struct SymbolTile;

/// Columns a disc's blended edge reaches past its diameter on each side.
const DISC_EDGE: usize = 1;

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    pub symbol: Symbol,
    pub tone: Tone,
    /// The diameter of the disc under it, in units.
    pub disc: Option<u8>,
    /// Shown only once Ride done's ride is saved.
    pub saved_only: bool,
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
            disc: None,
            saved_only: false,
        },
        Paused = 64 => Spec {
            name: "PAUSED",
            symbol: Symbol::Pause,
            tone: Tone::Warn,
            disc: Some(LAYOUT.space.badge),
            saved_only: false,
        },
        Stopping = 65 => Spec {
            name: "STOPPING",
            symbol: Symbol::Stop,
            tone: Tone::Bad,
            disc: Some(LAYOUT.space.badge),
            saved_only: false,
        },
        Saved = 66 => Spec {
            name: "SAVED",
            symbol: Symbol::Tick,
            tone: Tone::Good,
            disc: Some(LAYOUT.space.small_disc),
            saved_only: true,
        },
    }
}

impl Tile for SymbolTile {
    type Arg = Mark;
    const NAME: &'static str = "symbol";

    fn forms(mark: Mark, at: &Scaled) -> Forms {
        let spec = mark.spec();
        let ink = at.art.symbol(spec.symbol).ink(0, 0);
        let (width, height) = match spec.disc {
            Some(disc) => (px(at, disc) + 2 * DISC_EDGE, px(at, disc) + 2 * DISC_EDGE),
            None => (ink.width(), ink.height()),
        };
        forms(&[Form {
            width: width as u16,
            height: height as u16,
            shapes: 1 + spec.disc.is_some() as u8,
            runs: 0,
        }])
    }

    fn prepare(mark: Mark, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let (spec, theme) = (mark.spec(), data.theme());
        let saved = data.done().map(|done| done.kept()) == Some(Kept::Saved);
        if spec.saved_only && !saved {
            return;
        }
        let icon = at.art.symbol(spec.symbol);
        let ink = icon.ink(0, 0);
        let rect = placed.rect.rect();
        let (cx, cy) = (rect.left + rect.right, rect.top + rect.bottom);
        let color = match spec.disc {
            Some(disc) => {
                out.shape(Shape::Disc {
                    cx: 2 * cx as i32,
                    cy: 2 * cy as i32,
                    radius: 2 * px(at, disc) as i32,
                    color: theme.tone(spec.tone),
                });
                theme.on_tone
            }
            None => theme.tone(spec.tone),
        };
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: (cx - ink.width()) / 2 - ink.left,
            top: (cy - ink.height()) / 2 - ink.top,
            color,
        });
    }
}
