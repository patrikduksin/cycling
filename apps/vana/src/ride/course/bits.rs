//! Test-only hashing of whole states, each `f32` by its bits, for
//! `testing::explore`: `f32` has no `Hash`, so the course types cannot
//! derive it.
use crate::ride::breadcrumb::Crumb;
use crate::ride::course::follow::Placement;
use crate::ride::course::turns::Turn;
use core::hash::{Hash, Hasher};
use std::vec::Vec;

pub(crate) trait Bits {
    fn bits(&self, into: &mut dyn Hasher);
}

impl Bits for f32 {
    fn bits(&self, into: &mut dyn Hasher) {
        into.write_u32(self.to_bits());
    }
}

macro_rules! by_hash {
    ($($ty:ty),*) => {$(
        impl Bits for $ty {
            fn bits(&self, mut into: &mut dyn Hasher) {
                self.hash(&mut into);
            }
        }
    )*};
}
by_hash!(bool, u8, u16, u32, u64, usize, Placement, Turn);

impl<A: Bits, B: Bits> Bits for (A, B) {
    fn bits(&self, into: &mut dyn Hasher) {
        self.0.bits(into);
        self.1.bits(into);
    }
}

impl<T: Bits> Bits for Option<T> {
    fn bits(&self, into: &mut dyn Hasher) {
        self.is_some().bits(into);
        if let Some(value) = self {
            value.bits(into);
        }
    }
}

impl<T: Bits, const N: usize> Bits for [T; N] {
    fn bits(&self, into: &mut dyn Hasher) {
        for value in self {
            value.bits(into);
        }
    }
}

impl<T: Bits> Bits for [T] {
    fn bits(&self, into: &mut dyn Hasher) {
        self.len().bits(into);
        for value in self {
            value.bits(into);
        }
    }
}

impl<T: Bits> Bits for Vec<T> {
    fn bits(&self, into: &mut dyn Hasher) {
        self.as_slice().bits(into);
    }
}

/// Takes a struct apart field by field, so a field added fails here.
macro_rules! fields {
    ($($ty:ident { $($field:ident),* $(,)? })*) => {$(
        impl $crate::ride::course::bits::Bits for $ty {
            fn bits(&self, into: &mut dyn core::hash::Hasher) {
                let $ty { $($field),* } = self;
                $($crate::ride::course::bits::Bits::bits($field, into);)*
            }
        }
    )*};
}
pub(crate) use fields;

fields! {
    Crumb { east, north, along }
}
