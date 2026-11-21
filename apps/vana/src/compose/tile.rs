use super::data::Data;
use super::input::Action;
use super::solve::{Placed, Rect16};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;
use core::fmt;
use core::marker::PhantomData;
use firmware_shell::rendering::font::Face;

pub const MAX_FORMS: usize = 6;

/// One look of a tile: the box its drawing covers and what it costs the
/// canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Form {
    pub width: u16,
    pub height: u16,
    pub shapes: u8,
    pub runs: u8,
}

impl Form {
    /// One line of `text` in `face`, drawn with its baseline on the box's
    /// bottom: the columns it may cover and the rows above its baseline.
    pub const fn text(face: &Face, text: &[u8]) -> Self {
        Self {
            width: face.fit_width(text) as u16,
            height: face.fit_height() as u16,
            shapes: 0,
            runs: 1,
        }
    }
}

/// A tile's looks, best first. Each is no larger than the one before it in
/// either direction, so the last is the tile's minimum.
pub type Forms = heapless::Vec<Form, MAX_FORMS>;

/// `list` as forms; a tile offers no more than `MAX_FORMS`.
pub fn forms(list: &[Form]) -> Forms {
    list.iter().copied().collect()
}

pub trait Arg: Copy {
    fn decode(raw: u8) -> Option<Self>;
    fn encode(self) -> u8;
    /// How a reason names the setting, such as `SPEED`.
    fn label(self) -> &'static str {
        ""
    }
}

impl Arg for () {
    fn decode(raw: u8) -> Option<Self> {
        (raw == 0).then_some(())
    }

    fn encode(self) -> u8 {
        0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Landed(pub Rect16);

pub trait Tile: Sync + 'static {
    type Arg: Arg;
    /// The tile's name in the phone's catalogue, the twin and the trace.
    const NAME: &'static str;
    fn forms(arg: Self::Arg, at: &Scaled) -> Forms;
    /// Draw the look `placed.form` inside `placed.rect`.
    fn prepare(arg: Self::Arg, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas);
    fn action(_: Self::Arg) -> Option<Action> {
        None
    }
    /// Where the tile's gliding graphic lands for `data`; a glide needs a
    /// frame only while this moves.
    fn glide(_: Self::Arg, _: &Placed, _: &Scaled, _: &dyn Data) -> Option<Landed> {
        None
    }
    /// A map or a chart: one a screen.
    fn heavy() -> bool {
        false
    }
}

pub trait Component: Sync {
    fn name(&self) -> &'static str;
    fn label(&self, arg: u8, out: &mut dyn fmt::Write) -> fmt::Result;
    /// Whether `arg` decodes and encodes back to itself.
    fn accepts(&self, arg: u8) -> bool;
    fn forms(&self, arg: u8, at: &Scaled) -> Forms;
    fn prepare(&self, arg: u8, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas);
    fn action(&self, arg: u8) -> Option<Action>;
    fn glide(&self, arg: u8, placed: &Placed, at: &Scaled, data: &dyn Data) -> Option<Landed>;
    fn heavy(&self) -> bool;
}

pub struct Erased<T>(PhantomData<T>);

impl<T> Erased<T> {
    pub const NEW: Self = Self(PhantomData);
}

impl<T: Tile> Component for Erased<T> {
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn label(&self, arg: u8, out: &mut dyn fmt::Write) -> fmt::Result {
        match T::Arg::decode(arg) {
            Some(arg) => out.write_str(arg.label()),
            None => write!(out, "{arg}"),
        }
    }

    fn accepts(&self, arg: u8) -> bool {
        T::Arg::decode(arg).is_some_and(|decoded| decoded.encode() == arg)
    }

    #[inline(never)]
    fn forms(&self, arg: u8, at: &Scaled) -> Forms {
        T::Arg::decode(arg).map_or_else(Forms::new, |arg| T::forms(arg, at))
    }

    #[inline(never)]
    fn prepare(&self, arg: u8, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if let Some(arg) = T::Arg::decode(arg) {
            T::prepare(arg, placed, at, data, out);
        }
    }

    fn action(&self, arg: u8) -> Option<Action> {
        T::action(T::Arg::decode(arg)?)
    }

    fn glide(&self, arg: u8, placed: &Placed, at: &Scaled, data: &dyn Data) -> Option<Landed> {
        T::glide(T::Arg::decode(arg)?, placed, at, data)
    }

    fn heavy(&self) -> bool {
        T::heavy()
    }
}
