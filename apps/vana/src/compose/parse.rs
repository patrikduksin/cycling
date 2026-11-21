//! Accept: the only way to a valid [`Screen`], [`Layer`] or [`Screens`].
//! A config is accepted only if every tree solves on this panel, clear of
//! the corner cues its screen shows, under every combination of the flags
//! it mentions; then solving and drawing trust it. Before any tree solves,
//! the script tiles the record names get their instances (see
//! [`script::ready`]), so their looks are known.
//!
//! The record: the catalogue version, the screens the rider changed, each
//! `[key][count][count × node]`, then the layers, each
//! `[anchor << 4 | mode][event][hold][count][count × node]`.
use super::input::{Buttons, Controls, Cues};
use super::node::{Flags, MAX_DEPTH, MAX_NODES, Node, What, When};
use super::registry::{self, SCRIPT};
use super::reject::{Place, Reject, Why};
use super::script::{self, INSTANCES, SLOTS, TILES_A_TREE};
use super::solve::{self, Balance, Placed, Rect16, Scroll, Sink, Solved};
use crate::screens::scaled::Scaled;
use core::num::NonZeroU8;
use core::ops::Range;

pub const VERSION: u8 = 2;
pub const RECORD: usize = firmware_services::storage::CAPACITY;
pub const MAX_SCREENS: usize = 32;
pub const MAX_LAYERS: usize = 8;

/// A tree whose shape, kinds and settings parsed, for one panel; only
/// [`tree`] makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    nodes: heapless::Vec<Node, MAX_NODES>,
    at: Scaled,
}

impl Tree {
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub const fn at(&self) -> &Scaled {
        &self.at
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    key: u8,
    tree: Tree,
    controls: Controls,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Top,
    Bottom,
    Centre,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Floats over the tiles, which stay put.
    Overlay,
    /// Takes a band, and the tiles re-tile under it.
    Push,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    TurnSoon,
    ClimbStart,
    Lap,
    SensorLow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    Seconds(NonZeroU8),
    UntilEventEnds,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    anchor: Anchor,
    mode: Mode,
    event: Event,
    hold: Hold,
    tree: Tree,
}

/// One screen of a record changed: to an accepted tree, or back to its
/// built-in.
#[derive(Clone, Copy, Debug)]
pub enum Edit<'a> {
    Set(&'a Screen),
    Reset(u8),
}

impl Edit<'_> {
    const fn key(self) -> u8 {
        match self {
            Self::Set(screen) => screen.key,
            Self::Reset(key) => key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screens {
    screens: heapless::Vec<Screen, MAX_SCREENS>,
    layers: heapless::Vec<Layer, MAX_LAYERS>,
}

struct Check;

impl Sink<'_> for Check {
    fn start(&mut self, _: &Tree, _: Flags) {}
    fn leaf(&mut self, _: Placed, _: bool) {}
    fn scroll(&mut self, _: Scroll) {}
}

impl Screen {
    /// A screen the firmware ships, on one panel. It skips accept: a test
    /// accepts every built-in on every panel a device declares, so a
    /// built-in that does not fit fails the build gate, never a ride.
    pub fn builtin(key: u8, nodes: &[Node], at: Scaled, controls: Controls) -> Self {
        Self {
            key,
            tree: Tree {
                nodes: nodes.iter().copied().collect(),
                at,
            },
            controls,
        }
    }

    /// Accept screen `key` with the buttons and cues code gives it.
    pub fn parse(key: u8, bytes: &[u8], at: &Scaled, controls: Controls) -> Result<Self, Reject> {
        let place = Place::Screen(key);
        let tree = tree(bytes, at).map_err(|why| Reject { place, why })?;
        if let Some(why) = unreachable_tile(&tree, controls.buttons()) {
            return Err(Reject { place, why });
        }
        let cues = controls.cues();
        for flags in mentioned(&tree).subsets() {
            solve::solve(
                &tree,
                Rect16::of(at.screen()),
                flags,
                Balance::On,
                cues,
                &mut Check,
            )
            .map_err(|why| Reject { place, why })?;
        }
        Ok(Self {
            key,
            tree,
            controls,
        })
    }

    pub const fn key(&self) -> u8 {
        self.key
    }

    pub const fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The cues it was accepted with, which its tiles keep clear of.
    pub const fn cues(&self) -> Option<&Cues> {
        self.controls.cues()
    }

    pub const fn controls(&self) -> &Controls {
        &self.controls
    }

    /// Lay the screen out clear of the cues it was accepted with.
    pub fn solve<'a>(&'a self, flags: Flags, out: &mut Solved<'a>) {
        self.solve_in(Rect16::of(self.tree.at.screen()), flags, out);
    }

    fn solve_in<'a>(&'a self, area: Rect16, flags: Flags, out: &mut Solved<'a>) {
        let cues = self.controls.cues();
        let solved = solve::solve(&self.tree, area, flags, Balance::On, cues, out);
        if let Err(why) = solved {
            unreachable!(
                "accept solved this screen, with its cues, under every flag combination: {why}"
            );
        }
    }
}

/// The first tile with an action no button of its screen reaches where
/// focus never lands on it: anywhere on a screen that keeps MOVE, and in a
/// list's item template, which stands for every item rather than one.
fn unreachable_tile(tree: &Tree, buttons: &Buttons) -> Option<Why> {
    let items = items(&tree.nodes);
    tree.nodes.iter().enumerate().find_map(|(index, node)| {
        let What::Leaf { component, arg } = node.known() else {
            return None;
        };
        let action = registry::tile(component).action(arg)?;
        let node = index as u8;
        if buttons.reach(action) {
            None
        } else if buttons.move_.is_some() {
            Some(Why::NoButtonPath { node })
        } else {
            items.contains(&index).then_some(Why::ItemAction { node })
        }
    })
}

/// The nodes of the scroll's item template: the subtree after the scroll.
fn items(nodes: &[Node]) -> Range<usize> {
    let Some(scroll) = nodes
        .iter()
        .position(|node| matches!(node.known(), What::Scroll { .. }))
    else {
        return 0..0;
    };
    let (mut end, mut open) = (scroll + 1, 1);
    while open > 0 {
        open = open - 1 + usize::from(nodes[end].children());
        end += 1;
    }
    scroll + 1..end
}

impl Layer {
    fn parse(index: u8, head: [u8; 3], bytes: &[u8], at: &Scaled) -> Result<Self, Reject> {
        let place = Place::Layer(index);
        let refuse = |why| Reject { place, why };
        let anchor = Anchor::from_id(head[0] >> 4).ok_or(refuse(Why::Header(head[0])))?;
        let mode = Mode::from_id(head[0] & 0x0f).ok_or(refuse(Why::Header(head[0])))?;
        let event = Event::from_id(head[1]).ok_or(refuse(Why::Header(head[1])))?;
        if mode == Mode::Push && !matches!(anchor, Anchor::Top | Anchor::Bottom) {
            return Err(refuse(Why::Push));
        }
        let tree = tree(bytes, at).map_err(refuse)?;
        for flags in mentioned(&tree).subsets() {
            let band = solve::band(&tree, flags, anchor).map_err(refuse)?;
            solve::solve(&tree, band, flags, Balance::On, None, &mut Check).map_err(refuse)?;
        }
        Ok(Self {
            anchor,
            mode,
            event,
            hold: Hold::from_byte(head[2]),
            tree,
        })
    }

    pub const fn anchor(&self) -> Anchor {
        self.anchor
    }

    pub const fn mode(&self) -> Mode {
        self.mode
    }

    pub const fn event(&self) -> Event {
        self.event
    }

    pub const fn hold(&self) -> Hold {
        self.hold
    }

    pub const fn tree(&self) -> &Tree {
        &self.tree
    }

    fn band(&self, flags: Flags) -> Rect16 {
        match solve::band(&self.tree, flags, self.anchor) {
            Ok(band) => band,
            Err(why) => {
                unreachable!("accept placed this layer under every flag combination: {why}")
            }
        }
    }

    pub fn solve<'a>(&'a self, flags: Flags, out: &mut Solved<'a>) {
        let band = self.band(flags);
        let solved = solve::solve(&self.tree, band, flags, Balance::On, None, out);
        if let Err(why) = solved {
            unreachable!("accept solved this layer under every flag combination: {why}");
        }
    }
}

impl Screens {
    pub const EMPTY: Self = Self {
        screens: heapless::Vec::new(),
        layers: heapless::Vec::new(),
    };

    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn screen(&self, key: u8) -> Option<&Screen> {
        self.screens.iter().find(|screen| screen.key == key)
    }

    /// Lay out screen `key`, under the band of push layer `pushed` if one
    /// shows. False when the rider did not change that screen.
    pub fn solve<'a>(
        &'a self,
        key: u8,
        flags: Flags,
        pushed: Option<usize>,
        out: &mut Solved<'a>,
    ) -> bool {
        let Some(screen) = self.screen(key) else {
            return false;
        };
        let pushed = pushed.and_then(|index| self.layers.get(index));
        let area = match pushed {
            Some(layer) if layer.mode == Mode::Push => under(&screen.tree.at, layer.band(flags)),
            _ => Rect16::of(screen.tree.at.screen()),
        };
        screen.solve_in(area, flags, out);
        true
    }

    /// Write the record back: the bytes it was parsed from.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Reject> {
        self.write(None, out)
    }

    /// Write the record with one screen changed. The result is a record to
    /// accept, not an accepted one: the screens it holds may now be too many,
    /// or too long for `out`.
    pub fn edited(&self, edit: Edit<'_>, out: &mut [u8]) -> Result<usize, Reject> {
        self.write(Some(edit), out)
    }

    fn write(&self, edit: Option<Edit<'_>>, out: &mut [u8]) -> Result<usize, Reject> {
        let mut writer = Writer { out, at: 0 };
        let changed = edit.map(Edit::key);
        let kept = || {
            self.screens
                .iter()
                .filter(|screen| Some(screen.key) != changed)
        };
        let set = match edit {
            Some(Edit::Set(screen)) => Some(screen),
            Some(Edit::Reset(_)) | None => None,
        };
        let count = kept().count() + usize::from(set.is_some());
        writer.put(&[VERSION, count as u8]);
        for screen in kept().chain(set) {
            writer.put(&[screen.key, screen.tree.nodes.len() as u8]);
            writer.nodes(&screen.tree.nodes);
        }
        writer.put(&[self.layers.len() as u8]);
        for layer in &self.layers {
            let head = (layer.anchor as u8) << 4 | layer.mode as u8;
            writer.put(&[
                head,
                layer.event as u8,
                layer.hold.byte(),
                layer.tree.nodes.len() as u8,
            ]);
            writer.nodes(&layer.tree.nodes);
        }
        match writer.at <= writer.out.len() {
            true => Ok(writer.at),
            false => Err(Reject {
                place: Place::Record,
                why: Why::Long(writer.at),
            }),
        }
    }
}

/// Accept a whole record into `out`, which is left empty on a refusal.
/// `controls_of` names each screen's buttons and cues, by key: what the
/// buttons do stays in code.
pub fn parse(
    bytes: &[u8],
    at: &Scaled,
    controls_of: &dyn Fn(u8) -> Controls,
    out: &mut Screens,
) -> Result<(), Reject> {
    *out = Screens::EMPTY;
    let result = parse_into(bytes, at, controls_of, out);
    if result.is_err() {
        *out = Screens::EMPTY;
        script::clear();
    }
    result
}

fn parse_into(
    bytes: &[u8],
    at: &Scaled,
    controls_of: &dyn Fn(u8) -> Controls,
    out: &mut Screens,
) -> Result<(), Reject> {
    let record = |why| Reject {
        place: Place::Record,
        why,
    };
    if bytes.len() > RECORD {
        return Err(record(Why::Long(bytes.len())));
    }
    let mut reader = Reader { bytes, at: 0 };
    let [version, count] = reader.take().ok_or(record(Why::Truncated))?;
    if version != VERSION {
        return Err(record(Why::Version(version)));
    }
    script::ready(&scripts(bytes).map_err(record)?, at).map_err(record)?;
    for _ in 0..count {
        let [key, nodes] = reader.take().ok_or(record(Why::Truncated))?;
        let nodes = reader.nodes(nodes).ok_or(record(Why::Truncated))?;
        if out.screen(key).is_some() {
            return Err(record(Why::DuplicateScreen(key)));
        }
        let screen = Screen::parse(key, nodes, at, controls_of(key))?;
        out.screens
            .push(screen)
            .map_err(|_| record(Why::Screens(count)))?;
    }
    let [count] = reader.take().ok_or(record(Why::Truncated))?;
    for index in 0..count {
        let [anchor, event, hold, nodes] = reader.take().ok_or(record(Why::Truncated))?;
        let nodes = reader.nodes(nodes).ok_or(record(Why::Truncated))?;
        let layer = Layer::parse(index, [anchor, event, hold], nodes, at)?;
        out.layers
            .push(layer)
            .map_err(|_| record(Why::Layers(count)))?;
    }
    if reader.at != bytes.len() {
        return Err(record(Why::Trailing));
    }
    for (index, layer) in out.layers.iter().enumerate() {
        for screen in &out.screens {
            beneath(screen, (index as u8, layer), at)?;
        }
    }
    Ok(())
}

/// Whether `screen` still solves with `layer` showing, re-tiled under its
/// band if it pushes, and the two together fit the one canvas they draw on.
fn beneath(screen: &Screen, (index, layer): (u8, &Layer), at: &Scaled) -> Result<(), Reject> {
    let refuse = |why| Reject {
        place: Place::Screen(screen.key),
        why,
    };
    let flags = mentioned(&screen.tree).with(mentioned(&layer.tree));
    for flags in flags.subsets() {
        let band = layer.band(flags);
        let area = match layer.mode {
            Mode::Push => under(at, band),
            Mode::Overlay => Rect16::of(at.screen()),
        };
        let cues = screen.cues();
        let below = solve::solve(&screen.tree, area, flags, Balance::On, cues, &mut Check)
            .map_err(refuse)?;
        let above = solve::solve(&layer.tree, band, flags, Balance::On, None, &mut Check)
            .map_err(refuse)?;
        let (shapes, runs) = (below.shapes + above.shapes, below.runs + above.runs);
        if shapes > solve::MAX_SHAPES || runs > solve::MAX_RUNS {
            return Err(refuse(Why::Beneath {
                layer: index,
                shapes,
                runs,
                flags,
            }));
        }
    }
    Ok(())
}

/// The script slots the record's trees name, read ahead of accept. A
/// record that ends early is left for accept to refuse.
fn scripts(bytes: &[u8]) -> Result<heapless::Vec<u8, INSTANCES>, Why> {
    let mut slots = heapless::Vec::new();
    let mut named = 0u8;
    let mut tree = |nodes: &[u8]| {
        let mut tiles = 0;
        for node in nodes.as_chunks::<4>().0 {
            if let Some(What::Leaf {
                component: SCRIPT,
                arg,
            }) = Node::from_bytes(*node).what()
            {
                tiles += 1;
                // A slot past the last is accept's to refuse, as a setting.
                if usize::from(arg) < SLOTS && !slots.contains(&arg) {
                    named += 1;
                    let _ = slots.push(arg);
                }
            }
        }
        if tiles > TILES_A_TREE {
            return Err(Why::ScriptTiles(tiles as u8));
        }
        if usize::from(named) > INSTANCES {
            return Err(Why::Scripts(named));
        }
        Ok(())
    };
    let mut reader = Reader { bytes, at: 1 };
    let Some([screens]) = reader.take() else {
        return Ok(slots);
    };
    for _ in 0..screens {
        let Some(nodes) = reader
            .take()
            .and_then(|[_, count]: [u8; 2]| reader.nodes(count))
        else {
            return Ok(slots);
        };
        tree(nodes)?;
    }
    let Some([layers]) = reader.take() else {
        return Ok(slots);
    };
    for _ in 0..layers {
        let Some(nodes) = reader
            .take()
            .and_then(|[.., count]: [u8; 4]| reader.nodes(count))
        else {
            return Ok(slots);
        };
        tree(nodes)?;
    }
    Ok(slots)
}

fn under(at: &Scaled, band: Rect16) -> Rect16 {
    let panel = Rect16::of(at.screen());
    if band.top == panel.top {
        Rect16 {
            top: band.bottom,
            ..panel
        }
    } else {
        Rect16 {
            bottom: band.top,
            ..panel
        }
    }
}

fn mentioned(tree: &Tree) -> Flags {
    tree.nodes.iter().fold(Flags::NONE, |flags, node| {
        flags.with(node.condition().mentions())
    })
}

pub(crate) fn tree(bytes: &[u8], at: &Scaled) -> Result<Tree, Why> {
    if !bytes.len().is_multiple_of(4) {
        return Err(Why::Truncated);
    }
    let count = bytes.len() / 4;
    if count == 0 || count > MAX_NODES {
        return Err(Why::Nodes(count.min(usize::from(u8::MAX)) as u8));
    }
    let nodes: heapless::Vec<Node, MAX_NODES> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(Node::from_bytes)
        .collect();
    let mut open: heapless::Vec<Open, MAX_DEPTH> = heapless::Vec::new();
    let (mut hero, mut heavy, mut scroll) = (false, false, false);
    for (i, &node) in nodes.iter().enumerate() {
        let at = i as u8;
        if i > 0 && open.is_empty() {
            return Err(Why::Shape { node: at });
        }
        let when = node.condition();
        if i == 0 && when != When::ALWAYS {
            return Err(Why::Root);
        }
        if when.needs().meets(when.forbids()) {
            return Err(Why::Never { node: at });
        }
        let what = node.what().ok_or(Why::Kind {
            node: at,
            kind: node.bytes()[0],
        })?;
        let children = match what {
            What::Row { children } | What::Col { children } | What::Stack { children } => {
                if children == 0 {
                    return Err(Why::Empty { node: at });
                }
                children
            }
            What::Scroll { .. } => {
                if core::mem::replace(&mut scroll, true) {
                    return Err(Why::Scroll { node: at });
                }
                1
            }
            What::Leaf { component, arg } => {
                let tile = registry::tile(component);
                if !tile.accepts(arg) {
                    return Err(Why::Arg { node: at, arg });
                }
                if tile.heavy() && core::mem::replace(&mut heavy, true) {
                    return Err(Why::Heavy { node: at });
                }
                0
            }
        };
        if node.is_hero() && (children > 0 || core::mem::replace(&mut hero, true)) {
            return Err(Why::Hero { node: at });
        }
        if let Some(parent) = open.last_mut() {
            parent.awaited -= 1;
        }
        let entered = Open {
            node: at,
            awaited: children,
        };
        open.push(entered).map_err(|_| Why::Depth { node: at })?;
        while open.last().is_some_and(|open| open.awaited == 0) {
            open.pop();
        }
    }
    if let Some(open) = open.last() {
        return Err(Why::Shape { node: open.node });
    }
    Ok(Tree { nodes, at: *at })
}

struct Open {
    node: u8,
    awaited: u8,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let taken = self.bytes.get(self.at..self.at + N)?;
        self.at += N;
        taken.try_into().ok()
    }

    fn nodes(&mut self, count: u8) -> Option<&'a [u8]> {
        let length = usize::from(count) * 4;
        let taken = self.bytes.get(self.at..self.at + length)?;
        self.at += length;
        Some(taken)
    }
}

/// Counts every byte put, and writes those that fit.
struct Writer<'a> {
    out: &'a mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) {
        if let Some(out) = self.out.get_mut(self.at..self.at + bytes.len()) {
            out.copy_from_slice(bytes);
        }
        self.at += bytes.len();
    }

    fn nodes(&mut self, nodes: &[Node]) {
        for node in nodes {
            self.put(&node.bytes());
        }
    }
}

impl Hold {
    const fn from_byte(byte: u8) -> Self {
        match NonZeroU8::new(byte) {
            Some(seconds) => Self::Seconds(seconds),
            None => Self::UntilEventEnds,
        }
    }

    const fn byte(self) -> u8 {
        match self {
            Self::Seconds(seconds) => seconds.get(),
            Self::UntilEventEnds => 0,
        }
    }
}

impl Anchor {
    const fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Top,
            1 => Self::Bottom,
            2 => Self::Centre,
            3 => Self::TopLeft,
            4 => Self::TopRight,
            5 => Self::BottomLeft,
            6 => Self::BottomRight,
            _ => return None,
        })
    }
}

impl Mode {
    const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Overlay),
            1 => Some(Self::Push),
            _ => None,
        }
    }
}

impl Event {
    const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::TurnSoon),
            1 => Some(Self::ClimbStart),
            2 => Some(Self::Lap),
            3 => Some(Self::SensorLow),
            _ => None,
        }
    }
}
