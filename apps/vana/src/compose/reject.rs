use super::node::{Flags, Node, What};
use super::registry;
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reject {
    pub place: Place,
    pub why: Why,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Record,
    Screen(u8),
    Layer(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Across,
    Down,
}

impl Axis {
    pub const fn cross(self) -> Self {
        match self {
            Self::Across => Self::Down,
            Self::Down => Self::Across,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Version(u8),
    Long(usize),
    Truncated,
    Trailing,
    Screens(u8),
    Layers(u8),
    DuplicateScreen(u8),
    /// A tree of no nodes, or of more than `MAX_NODES`.
    Nodes(u8),
    /// The node's children run past the tree, or the tree has nodes no
    /// container holds.
    Shape {
        node: u8,
    },
    Depth {
        node: u8,
    },
    Empty {
        node: u8,
    },
    Kind {
        node: u8,
        kind: u8,
    },
    Arg {
        node: u8,
        arg: u8,
    },
    Root,
    Never {
        node: u8,
    },
    /// A container marked hero, or a second hero.
    Hero {
        node: u8,
    },
    Heavy {
        node: u8,
    },
    Scroll {
        node: u8,
    },
    Formless {
        node: u8,
    },
    Push,
    Header(u8),
    Misfit(Misfit),
    Leaves {
        count: u8,
        flags: Flags,
    },
    Canvas {
        shapes: u16,
        runs: u16,
        flags: Flags,
    },
    /// A tile acts on a screen that keeps MOVE, and no button of the screen
    /// does what it does.
    NoButtonPath {
        node: u8,
    },
    /// A tile of the list's item template acts, and no button of the screen
    /// does what it does: focus steps past the template.
    ItemAction {
        node: u8,
    },
    /// The screen and a layer over it overflow the canvas they share.
    Beneath {
        layer: u8,
        shapes: u16,
        runs: u16,
        flags: Flags,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Misfit {
    pub node: u8,
    pub what: Node,
    pub axis: Axis,
    pub gets: u16,
    pub needs: u16,
    pub flags: Flags,
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.place {
            Place::Record => {}
            Place::Screen(key) => write!(f, "screen {key}: ")?,
            Place::Layer(index) => write!(f, "layer {index}: ")?,
        }
        fmt::Display::fmt(&self.why, f)
    }
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Version(version) => write!(f, "catalogue version {version} is not this build's"),
            Self::Long(length) => write!(f, "{length} bytes is longer than a record"),
            Self::Truncated => f.write_str("the record ends early"),
            Self::Trailing => f.write_str("bytes follow the record"),
            Self::Screens(count) => write!(f, "{count} screens is too many"),
            Self::Layers(count) => write!(f, "{count} layers is too many"),
            Self::DuplicateScreen(key) => write!(f, "screen {key} is given twice"),
            Self::Nodes(count) => write!(f, "a tree of {count} nodes"),
            Self::Shape { node } => write!(f, "node {node}'s children do not match the tree"),
            Self::Depth { node } => write!(f, "node {node} nests too deep"),
            Self::Empty { node } => write!(f, "node {node} holds nothing"),
            Self::Kind { node, kind } => write!(f, "node {node} is of unknown kind {kind}"),
            Self::Arg { node, arg } => write!(f, "node {node} refuses setting {arg}"),
            Self::Root => f.write_str("the root shows under a condition"),
            Self::Never { node } => write!(f, "node {node} needs a flag it forbids"),
            Self::NoButtonPath { node } => {
                write!(f, "node {node} acts where no button of its screen reaches")
            }
            Self::ItemAction { node } => {
                write!(
                    f,
                    "node {node} is a list item that acts where no button reaches"
                )
            }
            Self::Hero { node } => write!(f, "node {node} cannot be the hero"),
            Self::Heavy { node } => write!(f, "node {node} is a second map or chart"),
            Self::Scroll { node } => write!(f, "node {node} is a second scroll"),
            Self::Formless { node } => write!(f, "node {node} has no look"),
            Self::Push => f.write_str("only a top or bottom band can push the tiles"),
            Self::Header(byte) => write!(f, "layer byte {byte} names nothing"),
            Self::Misfit(misfit) => fmt::Display::fmt(&misfit, f),
            Self::Leaves { count, flags } => {
                write!(f, "{count} tiles show")?;
                with(f, flags)
            }
            Self::Canvas {
                shapes,
                runs,
                flags,
            } => {
                write!(f, "the looks need {shapes} shapes and {runs} runs")?;
                with(f, flags)
            }
            Self::Beneath {
                layer,
                shapes,
                runs,
                flags,
            } => {
                write!(
                    f,
                    "with layer {layer} over it the looks need {shapes} shapes and {runs} runs"
                )?;
                with(f, flags)
            }
        }
    }
}

impl fmt::Display for Misfit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node {} ", self.node)?;
        name(self.what, f)?;
        let axis = match self.axis {
            Axis::Across => "wide",
            Axis::Down => "tall",
        };
        write!(f, " gets {} px {axis}, needs {}", self.gets, self.needs)?;
        with(f, self.flags)
    }
}

fn with(f: &mut fmt::Formatter<'_>, flags: Flags) -> fmt::Result {
    if flags == Flags::NONE {
        Ok(())
    } else {
        write!(f, " (with {flags})")
    }
}

fn name(node: Node, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match node.what() {
        Some(What::Row { .. }) => f.write_str("row"),
        Some(What::Col { .. }) => f.write_str("col"),
        Some(What::Scroll { .. }) => f.write_str("scroll"),
        Some(What::Stack { .. }) => f.write_str("stack"),
        Some(What::Leaf { component, arg }) => {
            let tile = registry::tile(component);
            f.write_str(tile.name())?;
            f.write_str("(")?;
            tile.label(arg, f)?;
            f.write_str(")")
        }
        None => f.write_str("node"),
    }
}
