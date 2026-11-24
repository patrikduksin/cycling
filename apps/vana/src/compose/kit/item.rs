//! A big rounded button, as Apple Watch draws a choice: a symbol and a
//! label centred as one group, in its tone on a raised fill, or white on a
//! fill of its tone. ACT or a tap runs its action; the focused one shows a
//! ring in the focus colour.
use super::{px, specs};
use crate::compose::data::Data;
use crate::compose::input::{Action, OnPage};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Align;
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Tone};
use device_api::input::Role;

pub struct Item;

#[derive(Debug)]
pub struct Spec {
    pub label: &'static str,
    pub symbol: Option<Symbol>,
    pub tone: Tone,
    /// Filled with its tone, white words on it; else raised, words in its
    /// tone.
    pub solid: bool,
    pub action: Action,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.label
    }
}

specs! {
    /// A button. Ids: home and sheet 0–63, pause, end and done 64–127,
    /// sensors 128–191.
    pub enum Button: Spec {
        End = 64 => Spec {
            label: "END",
            symbol: Some(Symbol::Stop),
            tone: Tone::Bad,
            solid: false,
            action: Action::Ride(Role::Move),
        },
        Resume = 65 => Spec {
            label: "RESUME",
            symbol: Some(Symbol::Play),
            tone: Tone::Good,
            solid: true,
            action: Action::Ride(Role::Act),
        },
        Back = 66 => Spec {
            label: "BACK",
            symbol: Some(Symbol::Chevron),
            tone: Tone::Text,
            solid: false,
            action: Action::Ride(Role::Move),
        },
        Save = 67 => Spec {
            label: "END",
            symbol: Some(Symbol::Check),
            tone: Tone::Bad,
            solid: true,
            action: Action::Ride(Role::Act),
        },
        Forget = 128 => Spec {
            label: "FORGET",
            symbol: Some(Symbol::Trash),
            tone: Tone::Bad,
            solid: false,
            action: Action::Page(OnPage::Forget),
        },
        Zero = 129 => Spec {
            label: "ZERO",
            symbol: Some(Symbol::Target),
            tone: Tone::Good,
            solid: true,
            action: Action::Page(OnPage::Calibrate),
        },
        Keep = 130 => Spec {
            label: "BACK",
            symbol: Some(Symbol::Chevron),
            tone: Tone::Text,
            solid: false,
            action: Action::Page(OnPage::Back),
        },
        ForgetNow = 131 => Spec {
            label: "FORGET",
            symbol: Some(Symbol::Trash),
            tone: Tone::Bad,
            solid: true,
            action: Action::Page(OnPage::Answer(true)),
        },
    }
}

fn shared_width(at: &Scaled) -> usize {
    let widest = Button::ALL.iter().map(|button| {
        let (symbol, label) = group(button.spec(), at);
        symbol + label
    });
    2 * px(at, LAYOUT.space.gutter) + widest.max().unwrap_or(0)
}

/// The symbol's ink width and the label's, as one centred group.
fn group(spec: &Spec, at: &Scaled) -> (usize, usize) {
    let symbol = spec.symbol.map_or(0, |symbol| {
        at.art.symbol(symbol).ink(0, 0).width() + px(at, LAYOUT.space.tight)
    });
    (
        symbol,
        at.face(LAYOUT.face.label).fit_width(spec.label.as_bytes()),
    )
}

fn draw(
    button: Button,
    placed: &Placed,
    at: &Scaled,
    data: &dyn Data,
    focused: bool,
    out: &mut Canvas,
) {
    let spec = button.spec();
    let theme = data.theme();
    let rect = placed.rect.rect();
    let radius = px(at, LAYOUT.radius.button);
    let (fill, ink) = if spec.solid {
        (theme.tone(spec.tone), theme.on_tone)
    } else {
        (theme.raised, theme.tone(spec.tone))
    };
    let inside = if focused {
        let ring = px(at, LAYOUT.space.ring);
        out.shape(Shape::Panel {
            rect,
            radius,
            color: theme.focus,
        });
        (rect.inset(ring), radius - ring)
    } else {
        (rect, radius)
    };
    out.shape(Shape::Panel {
        rect: inside.0,
        radius: inside.1,
        color: fill,
    });
    let (symbol, label) = group(spec, at);
    let label_face = at.face(LAYOUT.face.label);
    let middle = (rect.top + rect.bottom) / 2;
    let mut x = (rect.left + rect.right).saturating_sub(symbol + label) / 2;
    if let Some(symbol) = spec.symbol {
        let icon = at.art.symbol(symbol);
        let ink_box = icon.ink(0, 0);
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: x.saturating_sub(ink_box.left),
            top: middle.saturating_sub(ink_box.top + ink_box.height() / 2),
            color: ink,
        });
        x += ink_box.width() + px(at, LAYOUT.space.tight);
    }
    let base = middle + label_face.cap as usize / 2;
    out.fit(
        &[label_face],
        spec.label.as_bytes(),
        rect.right.saturating_sub(x),
        x,
        base,
        Align::Left,
        ink,
    );
}

impl Tile for Item {
    type Arg = Button;
    const NAME: &'static str = "item";

    fn forms(button: Button, at: &Scaled) -> Forms {
        let spec = button.spec();
        forms(&[Form {
            width: shared_width(at) as u16,
            height: px(at, LAYOUT.space.button) as u16,
            shapes: 2 + spec.symbol.is_some() as u8,
            runs: 1,
        }])
    }

    fn prepare(button: Button, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        draw(button, placed, at, data, false, out);
    }

    fn focused(button: Button, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        draw(button, placed, at, data, true, out);
    }

    fn action(button: Button) -> Option<Action> {
        Some(button.spec().action)
    }
}
