use super::*;
use crate::ride::course::bits::Bits;
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
    unplaced: bool,
    held: bool,
    join_fixes: u8,
    join_end: usize,
    turn_around_fixes: u8,
    last_fix: Option<Point>,
    following: bool,
    segment: usize,
    matched: Option<Point>,
    to_end_m: f32,
    furthest_to_end_m: f32,
    matched_fix: Option<Point>,
    search_whole_course: bool,
    heading: Option<u16>,
    taken_loop_ms: u64,
    turn_count: usize,
    scan: PlacedScan,
    nav: Navigator,
}

impl Owned {
    pub(super) fn encoding(&self) -> Vec<u8> {
        let mut encoding = ByteCollector::default();
        self.encode(&mut encoding);
        encoding.0
    }

    pub(super) fn view(&self) -> View<'_> {
        View {
            line: &self.line,
            len: self.len,
            turns: &self.turns,
            turn_count: self.turn_count,
            nav: &self.nav,
        }
    }
}
