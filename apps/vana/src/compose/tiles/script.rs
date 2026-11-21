use crate::compose::data::Data;
use crate::compose::script::{self, SLOTS};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;

/// A tile a WebAssembly module draws: its setting is the slot the module
/// was installed in (see [`script`]).
pub struct Script;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot(u8);

const LABELS: [&str; SLOTS] = ["SLOT 0", "SLOT 1", "SLOT 2", "SLOT 3"];

impl Arg for Slot {
    fn decode(raw: u8) -> Option<Self> {
        (usize::from(raw) < SLOTS).then_some(Self(raw))
    }

    fn encode(self) -> u8 {
        self.0
    }

    fn label(self) -> &'static str {
        LABELS[usize::from(self.0)]
    }
}

impl Tile for Script {
    type Arg = Slot;
    const NAME: &'static str = "script";

    fn forms(slot: Slot, _: &Scaled) -> Forms {
        script::forms(slot.0)
    }

    fn prepare(slot: Slot, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        script::draw(slot.0, placed, at, data, out);
    }
}
