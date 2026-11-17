use super::*;
use crate::ride::course::bits::{Bits, fields};
use core::hash::{Hash, Hasher};

#[derive(Default)]
pub(super) struct ByteCollector(Vec<u8>);

impl Hasher for ByteCollector {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn finish(&self) -> u64 {
        0
    }
}

impl Hash for Owned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::thread_local! {
            static BUFFER: core::cell::RefCell<ByteCollector> = core::cell::RefCell::default();
        }
        BUFFER.with_borrow_mut(|buffer| {
            buffer.0.clear();
            self.encode(buffer);
            state.write(&buffer.0);
        });
    }
}

fn same_crumb(a: &Crumb, b: &Crumb) -> bool {
    let bits =
        |Crumb { east, north, along }: &Crumb| (east.to_bits(), north.to_bits(), along.to_bits());
    bits(a) == bits(b)
}

fn tagged_line_bits(line: &[Crumb], into: &mut dyn Hasher) {
    let same = |a: &[Crumb], b: &[Crumb]| {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_crumb(a, b))
    };
    if line
        .iter()
        .all(|crumb| same_crumb(crumb, &Crumb::default()))
    {
        return into.write_u8(1);
    }
    for size in [Size::Issue, Size::Short] {
        for shape in Shape::ALL {
            if same(line, &fixture(size, shape).placed) {
                return into.write(&[2, size as u8, shape as u8]);
            }
        }
    }
    into.write_u8(0);
    line.bits(into);
}

/// A struct literal names every field, so a field added to `Follower`
/// fails here.
macro_rules! owned {
    ($($field:ident: $ty:ty),* $(,)?) => {
        #[derive(Clone)]
        pub(super) struct Owned {
            pub(super) line: Vec<Crumb>,
            pub(super) turns: Vec<Turn>,
            $(pub(super) $field: $ty),*
        }

        impl Owned {
            pub(super) fn of(follower: &Follower) -> Self {
                Self {
                    line: follower.line.to_vec(),
                    turns: follower.turns.to_vec(),
                    $($field: follower.$field),*
                }
            }

            pub(super) fn with<R>(&mut self, op: impl FnOnce(&mut Follower) -> R) -> R {
                let mut follower = Follower {
                    line: &mut self.line,
                    turns: &mut self.turns,
                    $($field: self.$field),*
                };
                let out = op(&mut follower);
                $(self.$field = follower.$field;)*
                out
            }

            fn encode(&self, into: &mut dyn Hasher) {
                tagged_line_bits(&self.line, into);
                self.turns.bits(into);
                $(self.$field.bits(into);)*
            }
        }
    };
}

owned! {
    len: usize,
    placement: Placement,
    life: Lifecycle,
    last_fix: Option<Point>,
    heading: Option<u16>,
    taken_loop_ms: u64,
    turn_count: usize,
    scan: PlacedScan,
}

impl Bits for Lifecycle {
    fn bits(&self, into: &mut dyn Hasher) {
        match self {
            Lifecycle::NoCourse => 0u8.bits(into),
            Lifecycle::Unplaced => 1u8.bits(into),
            Lifecycle::Joining(joining) => {
                2u8.bits(into);
                joining.bits(into);
            }
            Lifecycle::Following(following) => {
                3u8.bits(into);
                following.bits(into);
            }
            Lifecycle::Arrived { nav } => {
                4u8.bits(into);
                nav.bits(into);
            }
            Lifecycle::LetGo { awaits_frame } => {
                5u8.bits(into);
                awaits_frame.bits(into);
            }
        }
    }
}

fields! {
    Joining { fixes, against_fixes }
    Following {
        nav, segment, matched, matched_fix, to_end_m, furthest_to_end_m, search_whole_course
    }
}

impl Owned {
    pub(super) fn encoding(&self) -> Vec<u8> {
        let mut encoding = ByteCollector::default();
        self.encode(&mut encoding);
        encoding.0
    }

    pub(super) fn nav(&self) -> Navigator {
        self.life.nav().copied().unwrap_or_default()
    }

    pub(super) fn view(&self) -> View<'_> {
        View {
            line: &self.line,
            len: self.len,
            turns: &self.turns,
            turn_count: self.turn_count,
            nav: self.nav(),
        }
    }
}
