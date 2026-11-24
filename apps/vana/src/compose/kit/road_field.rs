//! A climb page field that is a different field on each stretch of road.
use super::field::{Field, FieldTile};
use super::specs;
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::ride::climb::Stretch;
use crate::screens::scaled::Scaled;

pub struct RoadField;

#[derive(Debug)]
pub struct Slot {
    pub name: &'static str,
    pub flat: Field,
    pub climb: Field,
    pub course: Field,
}

impl Slot {
    pub const fn label(&self) -> &'static str {
        self.name
    }

    const fn field(&self, stretch: Stretch) -> Field {
        match stretch {
            Stretch::Flat => self.flat,
            Stretch::Climb => self.climb,
            Stretch::Course => self.course,
        }
    }
}

specs! {
    /// A road field. Ids: climb 0–63.
    pub enum RoadSlot: Slot {
        First = 0 => Slot {
            name: "ALTITUDE OR DIST",
            flat: Field::Altitude,
            climb: Field::ClimbDist,
            course: Field::ToTop,
        },
        Second = 1 => Slot {
            name: "ASCENT OR ELEV",
            flat: Field::Ascent,
            climb: Field::ClimbElev,
            course: Field::UpToGo,
        },
    }
}

impl Tile for RoadField {
    type Arg = RoadSlot;
    const NAME: &'static str = "road field";

    /// Every stretch's field asks for the same box (a test holds this).
    fn forms(slot: RoadSlot, at: &Scaled) -> Forms {
        FieldTile::forms(slot.spec().field(Stretch::Flat), at)
    }

    fn prepare(slot: RoadSlot, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let stretch = data.road().map_or(Stretch::Flat, |road| road.stretch);
        FieldTile::prepare(slot.spec().field(stretch), placed, at, data, out);
    }
}
