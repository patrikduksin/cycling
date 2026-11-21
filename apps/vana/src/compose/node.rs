//! A screen's config tree: four bytes a node, in preorder, a container's
//! children following it.
use super::data::ListKind;
use super::registry;
use core::num::NonZeroU8;

pub const MAX_NODES: usize = 32;
/// Levels a tree nests at most, the root's included.
pub const MAX_DEPTH: usize = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags(u8);

impl Flags {
    pub const NONE: Self = Self(0);
    pub const COURSE: Self = Self(1);
    pub const RADAR: Self = Self(2);
    pub const WORKOUT: Self = Self(4);
    pub const ARRIVED: Self = Self(8);
    const ALL: [(Self, &'static str); 4] = [
        (Self::COURSE, "COURSE"),
        (Self::RADAR, "RADAR"),
        (Self::WORKOUT, "WORKOUT"),
        (Self::ARRIVED, "ARRIVED"),
    ];

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn meets(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Every combination of these flags, none first.
    pub fn subsets(self) -> impl Iterator<Item = Self> {
        let all = self.0;
        let mut next = Some(0u8);
        core::iter::from_fn(move || {
            let this = next?;
            next = (this != all).then(|| this.wrapping_sub(all) & all);
            Some(Self(this))
        })
    }
}

impl core::fmt::Display for Flags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for (flag, name) in Self::ALL {
            if self.contains(flag) {
                f.write_str(if first { "" } else { "+" })?;
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

/// When a node shows: every flag of `needs` set and none of `forbids`. The
/// low nibble needs, the high nibble forbids.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct When(u8);

impl When {
    pub const ALWAYS: Self = Self(0);

    pub const fn new(needs: Flags, forbids: Flags) -> Self {
        Self((needs.0 & 0x0f) | (forbids.0 << 4))
    }

    pub const fn needs(self) -> Flags {
        Flags(self.0 & 0x0f)
    }

    pub const fn forbids(self) -> Flags {
        Flags(self.0 >> 4)
    }

    pub const fn holds(self, flags: Flags) -> bool {
        flags.contains(self.needs()) && !flags.meets(self.forbids())
    }

    pub const fn mentions(self) -> Flags {
        self.needs().with(self.forbids())
    }
}

/// The top bit says grow, the low seven bits carry the units or the weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Size(u8);

/// How a Row or a Col sizes a child along its axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sizing {
    /// Exactly n scaled units.
    Units(u8),
    /// Its look.
    Content,
    /// Its look, then `w` shares of the slack.
    Grow(NonZeroU8),
}

impl Size {
    const GROW: u8 = 0x80;
    pub const CONTENT: Self = Self(Self::GROW);

    pub const fn units(n: u8) -> Self {
        assert!(n < Self::GROW, "at most 127 units");
        Self(n)
    }

    pub const fn grow(weight: u8) -> Self {
        assert!(weight > 0 && weight < Self::GROW, "a weight from 1 to 127");
        Self(Self::GROW | weight)
    }

    pub const fn get(self) -> Sizing {
        if self.0 & Self::GROW == 0 {
            return Sizing::Units(self.0);
        }
        match NonZeroU8::new(self.0 & !Self::GROW) {
            Some(weight) => Sizing::Grow(weight),
            None => Sizing::Content,
        }
    }

    pub const fn weight(self) -> u8 {
        match self.get() {
            Sizing::Units(_) | Sizing::Content => 0,
            Sizing::Grow(weight) => weight.get(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Row {
        children: u8,
    },
    Col {
        children: u8,
    },
    /// A viewport on its parent's axis. Its one child is the item template,
    /// repeated per item of `list`.
    Scroll {
        list: ListKind,
    },
    /// Children sharing one box; the first one present shows.
    Stack {
        children: u8,
    },
    Leaf {
        component: u8,
        arg: u8,
    },
}

/// One node of a tree as stored: kind, arg, size, condition. The kind's top
/// bit marks the hero, the one tile the upgrade serves first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Node {
    kind: u8,
    arg: u8,
    size: Size,
    when: When,
}

const _: () = assert!(core::mem::size_of::<Node>() == 4);

impl Node {
    const HERO: u8 = 0x80;
    const ROW: u8 = 0;
    const COL: u8 = 1;
    const SCROLL: u8 = 2;
    const STACK: u8 = 3;
    const LEAF: u8 = 4;

    pub const fn row(children: u8, size: Size) -> Self {
        Self::of(Self::ROW, children, size)
    }

    pub const fn col(children: u8, size: Size) -> Self {
        Self::of(Self::COL, children, size)
    }

    pub const fn scroll(list: ListKind, size: Size) -> Self {
        Self::of(Self::SCROLL, list as u8, size)
    }

    pub const fn stack(children: u8, size: Size) -> Self {
        Self::of(Self::STACK, children, size)
    }

    pub const fn leaf(component: u8, arg: u8, size: Size) -> Self {
        Self::of(Self::LEAF + component, arg, size)
    }

    const fn of(kind: u8, arg: u8, size: Size) -> Self {
        Self {
            kind,
            arg,
            size,
            when: When::ALWAYS,
        }
    }

    pub const fn when(self, when: When) -> Self {
        Self { when, ..self }
    }

    pub const fn hero(self) -> Self {
        Self {
            kind: self.kind | Self::HERO,
            ..self
        }
    }

    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self {
            kind: bytes[0],
            arg: bytes[1],
            size: Size(bytes[2]),
            when: When(bytes[3]),
        }
    }

    pub const fn bytes(self) -> [u8; 4] {
        [self.kind, self.arg, self.size.0, self.when.0]
    }

    pub const fn size(self) -> Size {
        self.size
    }

    pub const fn condition(self) -> When {
        self.when
    }

    pub const fn is_hero(self) -> bool {
        self.kind & Self::HERO != 0
    }

    pub fn what(self) -> Option<What> {
        let arg = self.arg;
        Some(match self.kind & !Self::HERO {
            Self::ROW => What::Row { children: arg },
            Self::COL => What::Col { children: arg },
            Self::SCROLL => What::Scroll {
                list: ListKind::from_id(arg)?,
            },
            Self::STACK => What::Stack { children: arg },
            kind => {
                let component = kind - Self::LEAF;
                registry::get(component)?;
                What::Leaf { component, arg }
            }
        })
    }

    pub(super) fn known(self) -> What {
        match self.what() {
            Some(what) => what,
            None => unreachable!("parse admits only known kinds"),
        }
    }

    pub(super) fn children(self) -> u8 {
        match self.known() {
            What::Row { children } | What::Col { children } | What::Stack { children } => children,
            What::Scroll { .. } => 1,
            What::Leaf { .. } => 0,
        }
    }
}
