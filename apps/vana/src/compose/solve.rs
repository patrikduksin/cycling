use super::data::{Data, ListKind};
use super::input::{Action, Cues};
use super::list::Costed;
use super::node::{Flags, MAX_DEPTH, MAX_NODES, Node, Sizing, What};
use super::parse::{Anchor, Tree};
use super::registry;
use super::reject::{Axis, Misfit, Why};
use super::tile::Component;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Rect;
use crate::screens::scaled::Scaled;

pub const MAX_LEAVES: usize = 24;
pub const MAX_SHAPES: u16 = 24;
pub const MAX_RUNS: u16 = 28;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect16 {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}

impl Rect16 {
    pub const fn width(self) -> u16 {
        self.right - self.left
    }

    pub const fn height(self) -> u16 {
        self.bottom - self.top
    }

    pub const fn rect(self) -> Rect {
        Rect {
            left: self.left as usize,
            top: self.top as usize,
            right: self.right as usize,
            bottom: self.bottom as usize,
        }
    }

    pub const fn of(rect: Rect) -> Self {
        Self {
            left: rect.left as u16,
            top: rect.top as u16,
            right: rect.right as u16,
            bottom: rect.bottom as u16,
        }
    }

    const fn start(self, axis: Axis) -> u16 {
        match axis {
            Axis::Across => self.left,
            Axis::Down => self.top,
        }
    }

    const fn end(self, axis: Axis) -> u16 {
        match axis {
            Axis::Across => self.right,
            Axis::Down => self.bottom,
        }
    }

    const fn len(self, axis: Axis) -> u16 {
        self.end(axis) - self.start(axis)
    }

    const fn with_span(self, axis: Axis, start: u16, end: u16) -> Self {
        match axis {
            Axis::Across => Self {
                left: start,
                right: end,
                ..self
            },
            Axis::Down => Self {
                top: start,
                bottom: end,
                ..self
            },
        }
    }
}

/// One tile of a solved layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Placed {
    /// The tile's box with the active corner cues cut out: the tile draws
    /// only here.
    pub rect: Rect16,
    /// The leaf's index in its tree.
    pub node: u8,
    /// The look shown, an index into the tile's forms; it fits `rect`.
    pub form: u8,
}

/// A Scroll's viewport and its item template, placed once at the
/// viewport's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scroll {
    pub node: u8,
    pub viewport: Rect16,
    pub template: Rect16,
    first: u8,
    count: u8,
}

impl Scroll {
    /// The page of items its viewport shows, on this panel.
    pub fn page(&self, at: &Scaled) -> crate::screens::scroll::Layout {
        super::list::page(self.viewport, self.template, at)
    }

    /// Where its template's tiles sit among the layout's tiles.
    pub fn tiles(&self) -> core::ops::Range<usize> {
        usize::from(self.first)..usize::from(self.first + self.count)
    }
}

/// A tree solved under one set of flags. Preorder of its tiles is the
/// focus order of those with an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solved<'a> {
    tree: Option<&'a Tree>,
    flags: Flags,
    leaves: heapless::Vec<Placed, MAX_LEAVES>,
    focus: u32,
    scroll: Option<Scroll>,
}

const _: () = assert!(core::mem::size_of::<Solved>() <= 340);

impl Solved<'_> {
    pub const EMPTY: Self = Self {
        tree: None,
        flags: Flags::NONE,
        leaves: heapless::Vec::new(),
        focus: 0,
        scroll: None,
    };

    pub const fn flags(&self) -> Flags {
        self.flags
    }

    pub fn leaves(&self) -> &[Placed] {
        &self.leaves
    }

    /// The scroll and the tiles of its template.
    pub fn scroll(&self) -> Option<(Scroll, &[Placed])> {
        let scroll = self.scroll?;
        Some((scroll, &self.leaves[scroll.tiles()]))
    }

    /// Draw each tile onto `out`, a scroll's template once a card.
    pub fn prepare(&self, data: &dyn Data, out: &mut Canvas) {
        if let Some(tree) = self.tree {
            prepare(tree, &self.leaves, self.scroll.as_ref(), None, data, out);
        }
    }

    /// What a tap or ACT on `placed`, a tile of this layout, asks for.
    pub fn action(&self, placed: &Placed) -> Option<Action> {
        let (tile, arg) = component(self.tree?.nodes()[usize::from(placed.node)])?;
        tile.action(arg)
    }

    pub fn focus_order(&self) -> impl Iterator<Item = &Placed> {
        let focus = self.focus;
        self.leaves
            .iter()
            .enumerate()
            .filter(move |(index, _)| focus & (1 << index) != 0)
            .map(|(_, placed)| placed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Balance {
    #[cfg(test)]
    Off,
    On,
}

pub(crate) trait Sink<'a> {
    fn start(&mut self, tree: &'a Tree, flags: Flags);
    fn leaf(&mut self, placed: Placed, focus: bool);
    fn scroll(&mut self, scroll: Scroll);
    fn node(&mut self, _node: u8, _rect: Rect16) {}
}

impl<'a> Sink<'a> for Solved<'a> {
    fn start(&mut self, tree: &'a Tree, flags: Flags) {
        *self = Self {
            tree: Some(tree),
            flags,
            ..Self::EMPTY
        };
    }

    fn leaf(&mut self, placed: Placed, focus: bool) {
        if focus {
            self.focus |= 1 << self.leaves.len();
        }
        if self.leaves.push(placed).is_err() {
            unreachable!("solve refuses more than MAX_LEAVES tiles before it places one");
        }
    }

    fn scroll(&mut self, scroll: Scroll) {
        self.scroll = Some(scroll);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cost {
    pub shown: u8,
    pub shapes: u16,
    pub runs: u16,
}

#[inline(never)]
pub(crate) fn solve<'a>(
    tree: &'a Tree,
    area: Rect16,
    flags: Flags,
    balance: Balance,
    cues: Option<&Cues>,
    sink: &mut impl Sink<'a>,
) -> Result<Cost, Why> {
    let at = tree.at();
    let mut solver = Solver::new(tree.nodes(), flags);
    solver.looks(at)?;
    solver.upgrade(at, area);
    sink.start(tree, flags);
    solver.place(at, area, balance, cues, sink)
}

/// Draw `leaves`, solved from `tree`, onto `out`: each tile once, but the
/// template of `scroll` once on each card in view.
/// Draw `leaves`; the tile at node `focused` shows its focus.
pub fn prepare(
    tree: &Tree,
    leaves: &[Placed],
    scroll: Option<&Scroll>,
    focused: Option<u8>,
    data: &dyn Data,
    out: &mut Canvas,
) {
    let template = scroll.map_or(0..0, Scroll::tiles);
    for (index, placed) in leaves.iter().enumerate() {
        if template.contains(&index) {
            continue;
        }
        if let Some((tile, arg)) = component(tree.nodes()[usize::from(placed.node)]) {
            if Some(placed.node) == focused {
                tile.focused(arg, placed, tree.at(), data, out);
            } else {
                tile.prepare(arg, placed, tree.at(), data, out);
            }
        }
    }
    if let Some(scroll) = scroll {
        super::list::prepare(tree, scroll, &leaves[template], data, out);
    }
}

pub(crate) fn band(tree: &Tree, flags: Flags, anchor: Anchor) -> Result<Rect16, Why> {
    let at = tree.at();
    let mut solver = Solver::new(tree.nodes(), flags);
    solver.looks(at)?;
    solver.measure(at);
    let [width, height] = solver.want[0];
    let (panel_width, panel_height) = (at.panel.width as u16, at.panel.height as u16);
    for (axis, wants, has) in [
        (Axis::Across, width, panel_width),
        (Axis::Down, height, panel_height),
    ] {
        if wants > has {
            return Err(solver.misfit(0, axis, has, wants));
        }
    }
    let width = match anchor {
        Anchor::Top | Anchor::Bottom => panel_width,
        _ => width,
    };
    let left = match anchor {
        Anchor::Centre => (panel_width - width) / 2,
        Anchor::TopRight | Anchor::BottomRight => panel_width - width,
        _ => 0,
    };
    let top = match anchor {
        Anchor::Centre => (panel_height - height) / 2,
        Anchor::Bottom | Anchor::BottomLeft | Anchor::BottomRight => panel_height - height,
        _ => 0,
    };
    Ok(Rect16 {
        left,
        top,
        right: left + width,
        bottom: top + height,
    })
}

const fn index(axis: Axis) -> usize {
    match axis {
        Axis::Across => 0,
        Axis::Down => 1,
    }
}

fn component(node: Node) -> Option<(&'static dyn Component, u8)> {
    match node.known() {
        What::Leaf { component, arg } => Some((registry::tile(component), arg)),
        _ => None,
    }
}

const fn saturate(pixels: u32) -> u16 {
    if pixels > u16::MAX as u32 {
        u16::MAX
    } else {
        pixels as u16
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnitBands {
    AllFit,
    SomeOverflow,
}

struct Solver<'a> {
    nodes: &'a [Node],
    flags: Flags,
    end: [u8; MAX_NODES],
    present: u32,
    never_upgraded: u32,
    want: [[u16; 2]; MAX_NODES],
    look: [u8; MAX_NODES],
}

impl<'a> Solver<'a> {
    fn new(nodes: &'a [Node], flags: Flags) -> Self {
        let mut solver = Self {
            nodes,
            flags,
            end: [0; MAX_NODES],
            present: 0,
            never_upgraded: 0,
            want: [[0; 2]; MAX_NODES],
            look: [0; MAX_NODES],
        };
        for i in (0..nodes.len()).rev() {
            let mut next = i + 1;
            for _ in 0..nodes[i].children() {
                next = usize::from(solver.end[next]);
            }
            solver.end[i] = next as u8;
            if let What::Scroll { .. } = nodes[i].known() {
                for item in i + 1..next {
                    solver.never_upgraded |= 1 << item;
                }
            }
        }
        solver.present = u32::from(nodes[0].condition().holds(flags));
        for i in 0..nodes.len() {
            if solver.is_present(i) {
                let mut child = i + 1;
                while child < usize::from(solver.end[i]) {
                    if nodes[child].condition().holds(flags) {
                        solver.present |= 1 << child;
                    }
                    child = usize::from(solver.end[child]);
                }
            }
        }
        solver
    }

    fn is_present(&self, i: usize) -> bool {
        self.present & (1 << i) != 0
    }

    fn children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let end = usize::from(self.end[i]);
        let mut child = i + 1;
        core::iter::from_fn(move || {
            while child < end {
                let this = child;
                child = usize::from(self.end[this]);
                if self.is_present(this) {
                    return Some(this);
                }
            }
            None
        })
    }

    /// An item's height down its scroll, which place refuses to lay
    /// across: the template's units when it names them, else its look.
    fn item_length(&self, template: usize, at: &Scaled) -> u16 {
        match self.nodes[template].size().get() {
            Sizing::Units(n) => at.unit(usize::from(n)) as u16,
            Sizing::Content | Sizing::Grow(_) => self.want[template][index(Axis::Down)],
        }
    }

    fn misfit(&self, node: usize, axis: Axis, gets: u16, needs: u16) -> Why {
        Why::Misfit(Misfit {
            node: node as u8,
            what: self.nodes[node],
            axis,
            gets,
            needs,
            flags: self.flags,
        })
    }

    fn looks(&mut self, at: &Scaled) -> Result<(), Why> {
        let mut count = 0;
        for i in 0..self.nodes.len() {
            let Some((tile, arg)) = component(self.nodes[i]) else {
                continue;
            };
            if !self.is_present(i) {
                continue;
            }
            count += 1;
            let forms = tile.forms(arg, at);
            let Some(smallest) = forms.last() else {
                return Err(Why::Formless { node: i as u8 });
            };
            self.want[i] = [smallest.width, smallest.height];
            self.look[i] = (forms.len() - 1) as u8;
        }
        if usize::from(count) > MAX_LEAVES {
            return Err(Why::Leaves {
                count,
                flags: self.flags,
            });
        }
        Ok(())
    }

    fn measure(&mut self, at: &Scaled) -> UnitBands {
        let mut bands = UnitBands::AllFit;
        for i in (0..self.nodes.len()).rev() {
            if !self.is_present(i) {
                continue;
            }
            let want = match self.nodes[i].known() {
                What::Leaf { .. } => continue,
                What::Row { .. } => self.line(i, Axis::Across, at, &mut bands),
                What::Col { .. } => self.line(i, Axis::Down, at, &mut bands),
                What::Stack { .. } => {
                    let mut want = [0, 0];
                    for child in self.children(i) {
                        want[0] = want[0].max(self.want[child][0]);
                        want[1] = want[1].max(self.want[child][1]);
                    }
                    want
                }
                // A card's slot holds its template, or what the list says
                // while empty, with gaps above and below and a gutter either
                // side.
                What::Scroll { list } => {
                    let (across, down) = (index(Axis::Across), index(Axis::Down));
                    let mut want = super::list::empty_size(list, at);
                    for child in self.children(i) {
                        want[across] = want[across].max(self.want[child][across]);
                        let item = self.want[child][down].max(self.item_length(child, at));
                        want[down] = want[down].max(item);
                    }
                    want[across] += 2 * super::list::gutter(at);
                    want[down] += 2 * super::list::gap(at);
                    want
                }
            };
            self.want[i] = want;
        }
        bands
    }

    fn line(&self, i: usize, axis: Axis, at: &Scaled, bands: &mut UnitBands) -> [u16; 2] {
        let (main, cross) = (index(axis), index(axis.cross()));
        let (mut units, mut pixels, mut across) = (0, 0u32, 0);
        for child in self.children(i) {
            match self.nodes[child].size().get() {
                Sizing::Units(n) => {
                    let before = units;
                    units += usize::from(n);
                    let gets = at.unit(units) - at.unit(before);
                    if usize::from(self.want[child][main]) > gets {
                        *bands = UnitBands::SomeOverflow;
                    }
                }
                Sizing::Content | Sizing::Grow(_) => pixels += u32::from(self.want[child][main]),
            }
            across = across.max(self.want[child][cross]);
        }
        let mut want = [0; 2];
        want[main] = saturate(at.unit(units) as u32 + pixels);
        want[cross] = across;
        want
    }

    fn fits(&mut self, at: &Scaled, area: Rect16) -> bool {
        self.measure(at) == UnitBands::AllFit
            && self.want[0][0] <= area.width()
            && self.want[0][1] <= area.height()
    }

    fn upgrade(&mut self, at: &Scaled, area: Rect16) {
        if !self.fits(at, area) {
            return;
        }
        let mut done = 0u32;
        while let Some(i) = self.next_upgrade(done) {
            done |= 1 << i;
            let Some((tile, arg)) = component(self.nodes[i]) else {
                continue;
            };
            let forms = tile.forms(arg, at);
            let kept = self.want[i];
            for (look, form) in forms.iter().enumerate().take(usize::from(self.look[i])) {
                self.want[i] = [form.width, form.height];
                if self.fits(at, area) {
                    self.look[i] = look as u8;
                    break;
                }
                self.want[i] = kept;
            }
        }
        self.measure(at);
    }

    fn next_upgrade(&self, done: u32) -> Option<usize> {
        let skip = done | self.never_upgraded | !self.present;
        let mut best: Option<(usize, (bool, u8))> = None;
        for (i, &node) in self.nodes.iter().enumerate() {
            if let Sizing::Units(_) = node.size().get() {
                continue;
            }
            if skip & (1 << i) != 0 || component(node).is_none() {
                continue;
            }
            let key = (node.is_hero(), node.size().weight());
            if best.is_none_or(|(_, top)| key > top) {
                best = Some((i, key));
            }
        }
        best.map(|(i, _)| i)
    }

    fn place(
        &self,
        at: &Scaled,
        area: Rect16,
        balance: Balance,
        cues: Option<&Cues>,
        sink: &mut impl Sink<'a>,
    ) -> Result<Cost, Why> {
        let mut frames: heapless::Vec<Frame, MAX_DEPTH> = heapless::Vec::new();
        let mut cost = Cost::default();
        let mut i = 0;
        while i < self.nodes.len() {
            while frames
                .last()
                .is_some_and(|frame| usize::from(frame.end) <= i)
            {
                if let Some(frame) = frames.pop() {
                    frame.close(at, &mut cost, sink)?;
                }
            }
            if !self.is_present(i) {
                i = usize::from(self.end[i]);
                continue;
            }
            let (rect, hidden, axis) = match frames.last_mut() {
                None => (area, false, Axis::Down),
                Some(frame) => {
                    let hidden = frame.hides();
                    (frame.child(self, i, at, &mut cost)?, hidden, frame.axis)
                }
            };
            sink.node(i as u8, rect);
            let node = self.nodes[i];
            let tile = component(node);
            if tile.is_some() || matches!(node.known(), What::Scroll { .. }) {
                for axis in [Axis::Across, Axis::Down] {
                    let (has, wants) = (rect.len(axis), self.want[i][index(axis)]);
                    if wants > has {
                        return Err(self.misfit(i, axis, has, wants));
                    }
                }
            }
            if let Some((tile, arg)) = tile {
                let before = cost;
                self.leaf(i, rect, hidden, (tile, arg), at, cues, &mut cost, sink)?;
                if let Some(frame) = frames.iter_mut().rev().find(|frame| frame.scrolls()) {
                    frame.template_tile(rect, before, cost)?;
                }
            } else {
                let how = Open { at, balance, cost };
                let frame = Frame::open(self, i, rect, hidden, axis, how);
                if frames.push(frame).is_err() {
                    unreachable!("a parsed tree nests at most MAX_DEPTH deep");
                }
            }
            i += 1;
        }
        while let Some(frame) = frames.pop() {
            frame.close(at, &mut cost, sink)?;
        }
        if cost.shapes > MAX_SHAPES || cost.runs > MAX_RUNS {
            return Err(Why::Canvas {
                shapes: cost.shapes,
                runs: cost.runs,
                flags: self.flags,
            });
        }
        Ok(cost)
    }

    #[allow(clippy::too_many_arguments)]
    fn leaf(
        &self,
        i: usize,
        rect: Rect16,
        hidden: bool,
        (tile, arg): (&'static dyn Component, u8),
        at: &Scaled,
        cues: Option<&Cues>,
        cost: &mut Cost,
        sink: &mut impl Sink<'a>,
    ) -> Result<(), Why> {
        let forms = tile.forms(arg, at);
        let largest_within = |within: Rect16| {
            forms
                .iter()
                .position(|form| form.width <= within.width() && form.height <= within.height())
        };
        let costed = forms[largest_within(rect).unwrap_or(usize::from(self.look[i]))];
        cost.shapes += u16::from(costed.shapes);
        cost.runs += u16::from(costed.runs);
        if hidden {
            return Ok(());
        }
        let clear = cues.map_or(rect, |cues| cues.clear(rect, at));
        let Some(form) = largest_within(clear) else {
            let narrowest = forms.iter().map(|form| form.width).min().unwrap_or(0);
            return Err(self.misfit(i, Axis::Across, clear.width(), narrowest));
        };
        cost.shown += 1;
        let placed = Placed {
            rect: clear,
            node: i as u8,
            form: form as u8,
        };
        sink.leaf(placed, tile.action(arg).is_some());
        Ok(())
    }
}

struct Frame {
    node: u8,
    end: u8,
    rect: Rect16,
    hidden: bool,
    axis: Axis,
    kind: Kind,
}

enum Kind {
    Line(Line),
    /// Only one slide draws, so the stack costs the canvas what its
    /// costliest slide does: `base` is the cost before it, `most` the
    /// largest slide's so far.
    Stack {
        shown: bool,
        base: Cost,
        most: Cost,
    },
    /// `base` is the cost before the template; `tiles` are the template's,
    /// which `list::most` charges for every card in view.
    Scroll {
        list: ListKind,
        first: u8,
        template: Rect16,
        base: Cost,
        tiles: heapless::Vec<Costed, { super::list::MOST_TEMPLATE_TILES }>,
    },
}

struct Line {
    units: usize,
    pixels: u32,
    given: u32,
    shared: u32,
    weight: u32,
    slack: u32,
    count: u32,
    seen: u32,
    gapped: bool,
}

#[derive(Clone, Copy)]
struct Open<'a> {
    at: &'a Scaled,
    balance: Balance,
    cost: Cost,
}

impl Frame {
    fn open(
        solver: &Solver,
        i: usize,
        rect: Rect16,
        hidden: bool,
        parent: Axis,
        how: Open,
    ) -> Self {
        let line = |axis| Kind::Line(Line::new(solver, i, axis, rect, how.balance, how.at));
        let (axis, kind) = match solver.nodes[i].known() {
            What::Row { .. } => (Axis::Across, line(Axis::Across)),
            What::Col { .. } => (Axis::Down, line(Axis::Down)),
            What::Stack { .. } => (
                parent,
                Kind::Stack {
                    shown: false,
                    base: how.cost,
                    most: Cost::default(),
                },
            ),
            What::Scroll { list } => (
                parent,
                Kind::Scroll {
                    list,
                    first: how.cost.shown,
                    template: super::list::template_at(rect, how.at),
                    base: how.cost,
                    tiles: heapless::Vec::new(),
                },
            ),
            What::Leaf { .. } => unreachable!("a leaf opens no frame"),
        };
        Self {
            node: i as u8,
            end: solver.end[i],
            rect,
            hidden,
            axis,
            kind,
        }
    }

    fn scrolls(&self) -> bool {
        matches!(self.kind, Kind::Scroll { .. })
    }

    /// Note a tile of the scroll's template: where it sits on its card and
    /// what it cost the canvas.
    fn template_tile(&mut self, rect: Rect16, before: Cost, after: Cost) -> Result<(), Why> {
        let Kind::Scroll {
            template, tiles, ..
        } = &mut self.kind
        else {
            unreachable!("only a scroll's frame notes its template's tiles");
        };
        let costed = Costed {
            top: rect.top - template.top,
            bottom: rect.bottom - template.top,
            shapes: (after.shapes - before.shapes) as u8,
            runs: (after.runs - before.runs) as u8,
        };
        tiles
            .push(costed)
            .map_err(|_| Why::Template { node: self.node })
    }

    fn hides(&self) -> bool {
        match self.kind {
            Kind::Stack { shown, .. } => self.hidden || shown,
            _ => self.hidden,
        }
    }

    fn child(
        &mut self,
        solver: &Solver,
        child: usize,
        at: &Scaled,
        cost: &mut Cost,
    ) -> Result<Rect16, Why> {
        let want = solver.want[child];
        let (rect, main) = (self.rect, self.axis);
        match &mut self.kind {
            Kind::Stack { shown, base, most } => {
                *shown = true;
                slide_ends(cost, *base, most);
                Ok(rect)
            }
            Kind::Scroll { template, .. } => {
                if main == Axis::Across {
                    return Err(Why::Across { node: self.node });
                }
                let item = solver.item_length(child, at);
                if let Sizing::Units(_) = solver.nodes[child].size().get()
                    && want[index(main)] > item
                {
                    return Err(solver.misfit(child, main, item, want[index(main)]));
                }
                // Place refused a viewport too small for the item, its gaps
                // and gutters before it opened this frame.
                template.bottom = template.top + item;
                Ok(*template)
            }
            Kind::Line(line) => line.next(solver, child, main, rect, at),
        }
    }

    fn close<'a>(self, at: &Scaled, cost: &mut Cost, sink: &mut impl Sink<'a>) -> Result<(), Why> {
        match self.kind {
            Kind::Stack { base, mut most, .. } => {
                slide_ends(cost, base, &mut most);
                cost.shapes += most.shapes;
                cost.runs += most.runs;
            }
            Kind::Scroll {
                list,
                first,
                template,
                base,
                tiles,
            } => {
                let items = super::list::in_view(self.rect, template, at);
                if items > list.most_in_view() {
                    return Err(Why::InView {
                        node: self.node,
                        items: items as u8,
                    });
                }
                let (shapes, runs) = super::list::most(list, self.rect, template, &tiles, at);
                cost.shapes = base.shapes + shapes;
                cost.runs = base.runs + runs;
                if !self.hidden {
                    sink.scroll(Scroll {
                        node: self.node,
                        viewport: self.rect,
                        template,
                        first,
                        count: cost.shown - first,
                    });
                }
            }
            Kind::Line(_) => {}
        }
        Ok(())
    }
}

/// Fold the slide just placed into the stack's costliest and take its cost
/// back off, so the next slide starts from the stack's `base`.
fn slide_ends(cost: &mut Cost, base: Cost, most: &mut Cost) {
    most.shapes = most.shapes.max(cost.shapes - base.shapes);
    most.runs = most.runs.max(cost.runs - base.runs);
    cost.shapes = base.shapes;
    cost.runs = base.runs;
}

impl Line {
    fn new(
        solver: &Solver,
        i: usize,
        axis: Axis,
        rect: Rect16,
        balance: Balance,
        at: &Scaled,
    ) -> Self {
        let (mut units, mut pixels, mut weight, mut count) = (0, 0u32, 0u32, 0u32);
        for child in solver.children(i) {
            match solver.nodes[child].size().get() {
                Sizing::Units(n) => units += usize::from(n),
                Sizing::Content => pixels += u32::from(solver.want[child][index(axis)]),
                Sizing::Grow(w) => {
                    pixels += u32::from(solver.want[child][index(axis)]);
                    weight += u32::from(w.get());
                }
            }
            count += 1;
        }
        let fixed = at.unit(units) as u32 + pixels;
        Self {
            units: 0,
            pixels: 0,
            given: 0,
            shared: 0,
            weight,
            slack: u32::from(rect.len(axis)).saturating_sub(fixed),
            count,
            seen: 0,
            gapped: balance == Balance::On && weight == 0,
        }
    }

    fn edge(&self, at: &Scaled) -> u32 {
        at.unit(self.units) as u32 + self.pixels + self.given
    }

    fn next(
        &mut self,
        solver: &Solver,
        child: usize,
        main: Axis,
        rect: Rect16,
        at: &Scaled,
    ) -> Result<Rect16, Why> {
        let start = u32::from(rect.start(main));
        let gap = if self.gapped {
            self.slack * (self.seen + 1) / (self.count + 1)
        } else {
            0
        };
        let from = start + self.edge(at) + gap;
        match solver.nodes[child].size().get() {
            Sizing::Units(n) => self.units += usize::from(n),
            Sizing::Content => self.pixels += u32::from(solver.want[child][index(main)]),
            Sizing::Grow(weight) => {
                self.pixels += u32::from(solver.want[child][index(main)]);
                self.shared += u32::from(weight.get());
                self.given = if self.shared == self.weight {
                    self.slack
                } else {
                    self.given + self.slack * u32::from(weight.get()) / self.weight
                };
            }
        }
        let to = start + self.edge(at) + gap;
        self.seen += 1;
        let end = u32::from(rect.end(main));
        if to > end && matches!(solver.nodes[child].size().get(), Sizing::Units(_)) {
            let gets = saturate(end.saturating_sub(from));
            return Err(solver.misfit(child, main, gets, saturate(to - from)));
        }
        let (from, to) = (from.min(end), to.min(end));
        Ok(rect.with_span(main, from as u16, to as u16))
    }
}
