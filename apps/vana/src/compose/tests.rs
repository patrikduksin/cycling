use super::bench::{MAIN, MAIN_BYTES};
use super::data::{Data, Effort, ListKind, Metric, Nothing, OnGauge, PageData};
use super::input::{Action, Button, Buttons, Controls, OnPage};
use super::list;
use super::node::{Flags, MAX_NODES, Node, Size, Sizing, What, When};
use super::parse::{self, Anchor, Hold, Layer, Screen, Screens, VERSION};
use super::registry::{
    self, CAPTION, EFFORT, GAUGE, ICON, READING, SPACE, TOTAL, UNIT, VALUE, ZONE,
};
use super::reject::{Axis, Misfit, Place, Reject, Why};
use super::solve::{self, Balance, MAX_RUNS, MAX_SHAPES, Placed, Rect16, Scroll, Sink, Solved};
use super::tiles::icon::Glyph;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Rect;
use crate::screens::cue::Icon;
use crate::screens::limits::PANELS;
use crate::screens::scaled::{BASE, Scaled, Track};
use crate::screens::workout::Text;
use crate::testing::Rng;
use core::num::NonZeroU8;
use device_api::input::Role;
use firmware_shell::rendering::font::{FaceRole, SPILL};
use std::boxed::Box;
use std::format;
use std::string::ToString;
use std::vec;
use std::vec::Vec;

mod scroll;

fn panels() -> Vec<Scaled> {
    PANELS
        .iter()
        .filter_map(|&(width, height)| Scaled::fitting(width, height))
        .collect()
}

fn bytes(nodes: &[Node]) -> Vec<u8> {
    nodes.iter().flat_map(|node| node.bytes()).collect()
}

fn no_cues(_: u8) -> Controls {
    Controls::FREE
}

fn tile(component: u8, arg: u8, size: Size) -> Node {
    Node::leaf(component, arg, size)
}

struct Seen<'a> {
    solved: Solved<'a>,
    rects: [Option<Rect16>; MAX_NODES],
}

impl Seen<'_> {
    fn new() -> Self {
        Self {
            solved: Solved::EMPTY,
            rects: [None; MAX_NODES],
        }
    }
}

impl<'a> Sink<'a> for Seen<'a> {
    fn start(&mut self, tree: &'a parse::Tree, flags: Flags) {
        self.solved.start(tree, flags);
        self.rects = [None; MAX_NODES];
    }

    fn leaf(&mut self, placed: Placed, focus: bool) {
        self.solved.leaf(placed, focus);
    }

    fn scroll(&mut self, scroll: Scroll) {
        Sink::scroll(&mut self.solved, scroll);
    }

    fn node(&mut self, node: u8, rect: Rect16) {
        self.rects[usize::from(node)] = Some(rect);
    }
}

fn whole(at: &Scaled) -> Rect16 {
    Rect16::of(at.screen())
}

fn ends(nodes: &[Node]) -> Vec<usize> {
    let mut end = vec![0; nodes.len()];
    for i in (0..nodes.len()).rev() {
        let mut next = i + 1;
        let children = match nodes[i].what().unwrap() {
            What::Row { children } | What::Col { children } | What::Stack { children } => children,
            What::Scroll { .. } => 1,
            What::Leaf { .. } => 0,
        };
        for _ in 0..children {
            next = end[next];
        }
        end[i] = next;
    }
    end
}

fn children(end: &[usize], i: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut child = i + 1;
    while child < end[i] {
        out.push(child);
        child = end[child];
    }
    out
}

fn forms_of(node: Node, at: &Scaled) -> super::tile::Forms {
    let What::Leaf { component, arg } = node.what().unwrap() else {
        panic!("not a leaf");
    };
    registry::tile(component).forms(arg, at)
}

#[derive(Clone, Debug)]
enum Tree {
    Leaf,
    Line {
        across: bool,
        children: Vec<(Size, Tree)>,
    },
}

fn min_along(tree: &Tree, across: bool, at: &Scaled) -> usize {
    match tree {
        Tree::Leaf => 0,
        Tree::Line {
            across: axis,
            children,
        } if *axis == across => {
            let mut units = 0;
            let mut pixels = 0;
            for (size, child) in children {
                match size.get() {
                    Sizing::Units(n) => units += usize::from(n),
                    Sizing::Content | Sizing::Grow(_) => pixels += min_along(child, across, at),
                }
            }
            at.unit(units) + pixels
        }
        Tree::Line { children, .. } => children
            .iter()
            .map(|(_, child)| min_along(child, across, at))
            .max()
            .unwrap_or(0),
    }
}

fn any_tree(rng: &mut Rng, depth: usize, nodes: &mut usize, leaves: &mut usize) -> Tree {
    *nodes += 1;
    if depth == 6 || *nodes > 26 || *leaves > 18 || (depth > 1 && rng.one_in(3)) {
        *leaves += 1;
        return Tree::Leaf;
    }
    let across = rng.one_in(2);
    let count = rng.range(1, 6) as usize;
    let mut children = Vec::new();
    for _ in 0..count {
        if *nodes >= 31 || *leaves >= 23 {
            break;
        }
        let child = any_tree(rng, depth + 1, nodes, leaves);
        let needs = min_along(&child, across, &BASE) as u64;
        let size = if needs == 0 && rng.one_in(2) {
            Size::grow(rng.range(1, 3) as u8)
        } else {
            Size::units((needs + rng.range(0, 24)).min(127) as u8)
        };
        children.push((size, child));
    }
    if children.is_empty() {
        *leaves += 1;
        return Tree::Leaf;
    }
    Tree::Line { across, children }
}

fn flatten(tree: &Tree, size: Size, out: &mut Vec<Node>) {
    match tree {
        Tree::Leaf => out.push(tile(SPACE, 0, size)),
        Tree::Line { across, children } => {
            let count = children.len() as u8;
            out.push(if *across {
                Node::row(count, size)
            } else {
                Node::col(count, size)
            });
            for (size, child) in children {
                flatten(child, *size, out);
            }
        }
    }
}

fn split(rect: Rect, across: bool, tracks: &[Track], at: &Scaled) -> Vec<Rect> {
    macro_rules! by_count {
        ($($n:literal)+) => {
            match tracks.len() {
                $($n => {
                    let tracks: [Track; $n] = tracks.try_into().unwrap();
                    if across {
                        rect.cols(tracks, Track::Unit(0), at).to_vec()
                    } else {
                        rect.rows(tracks, Track::Unit(0), at).to_vec()
                    }
                })+
                _ => unreachable!(),
            }
        };
    }
    by_count!(1 2 3 4 5 6)
}

fn expected(tree: &Tree, rect: Rect, at: &Scaled, out: &mut Vec<Rect>) -> bool {
    match tree {
        Tree::Leaf => {
            out.push(rect);
            true
        }
        Tree::Line { across, children } => {
            let units: usize = children
                .iter()
                .map(|(size, _)| match size.get() {
                    Sizing::Units(n) => usize::from(n),
                    Sizing::Content | Sizing::Grow(_) => 0,
                })
                .sum();
            let extent = if *across { rect.width() } else { rect.height() };
            if at.unit(units) > extent {
                return false;
            }
            let tracks: Vec<Track> = children
                .iter()
                .map(|(size, _)| match size.get() {
                    Sizing::Units(n) => Track::Unit(u16::from(n)),
                    Sizing::Content | Sizing::Grow(_) => Track::Fr(size.weight()),
                })
                .collect();
            let rects = split(rect, *across, &tracks, at);
            children
                .iter()
                .zip(rects)
                .all(|((_, child), rect)| expected(child, rect, at, out))
        }
    }
}

#[test]
fn without_balance_a_tree_of_units_and_grow_solves_as_split_does() {
    let (mut matched, mut refused) = (0, 0);
    for seed in 0..3000 {
        let mut rng = Rng::new(seed);
        let tree = any_tree(&mut rng, 1, &mut 0, &mut 0);
        let mut nodes = Vec::new();
        flatten(&tree, Size::grow(1), &mut nodes);
        for at in panels() {
            let mut want = Vec::new();
            let fits = expected(&tree, at.screen(), &at, &mut want);
            let tree = parse::tree(&bytes(&nodes), &at).unwrap();
            let mut solved = Solved::EMPTY;
            let result = solve::solve(
                &tree,
                whole(&at),
                Flags::NONE,
                Balance::Off,
                None,
                &mut solved,
            );
            if fits {
                assert!(result.is_ok(), "seed {seed} on {:?}: {result:?}", at.panel);
                let got: Vec<Rect> = solved.leaves().iter().map(|p| p.rect.rect()).collect();
                assert_eq!(got, want, "seed {seed} on {:?}", at.panel);
                matched += 1;
            } else {
                assert!(
                    matches!(result, Err(Why::Misfit(_))),
                    "seed {seed} on {:?}: split refuses, solve gives {result:?}",
                    at.panel
                );
                refused += 1;
            }
        }
    }
    assert!(
        matched > 5000 && refused > 100,
        "{matched} matched, {refused} refused"
    );
}

fn any_condition(rng: &mut Rng) -> When {
    if !rng.one_in(4) {
        return When::ALWAYS;
    }
    let flags = [Flags::COURSE, Flags::RADAR, Flags::WORKOUT, Flags::ARRIVED];
    let needs = rng.pick(&flags);
    if rng.one_in(2) {
        When::new(needs, Flags::NONE)
    } else {
        When::new(Flags::NONE, needs)
    }
}

fn any_leaf(rng: &mut Rng) -> (u8, u8) {
    let metric = rng.pick(&Metric::ALL) as u8;
    let effort = rng.pick(&[Effort::Power, Effort::Heart]) as u8;
    match rng.below(8) {
        0 | 1 => (VALUE, metric),
        2 => (CAPTION, metric),
        3 => (UNIT, metric),
        4 => (GAUGE, effort),
        5 => (ZONE, effort),
        6 => (ICON, rng.below(4) as u8),
        _ => (SPACE, 0),
    }
}

fn any_size(rng: &mut Rng) -> Size {
    match rng.below(10) {
        0 | 1 => Size::grow(rng.range(1, 3) as u8),
        2 => Size::units(rng.range(0, 60) as u8),
        _ => Size::CONTENT,
    }
}

fn any_screen(rng: &mut Rng, budget: usize) -> Vec<Node> {
    fn grow(rng: &mut Rng, depth: usize, size: Size, budget: usize, out: &mut Vec<Node>) {
        let room = budget.saturating_sub(out.len());
        if depth == 6 || room < 3 || (depth > 1 && rng.one_in(3)) {
            let (component, arg) = any_leaf(rng);
            out.push(tile(component, arg, size).when(any_condition(rng)));
            return;
        }
        let count = rng.range(1, 4).min(room as u64 - 1) as u8;
        let at = out.len();
        out.push(match rng.below(8) {
            0 => Node::stack(count, size),
            1..=3 => Node::row(count, size),
            _ => Node::col(count, size),
        });
        if depth > 1 {
            out[at] = out[at].when(any_condition(rng));
        }
        for _ in 0..count {
            let child = any_size(rng);
            grow(rng, depth + 1, child, budget, out);
        }
    }
    let mut nodes = Vec::new();
    grow(rng, 1, Size::grow(1), budget, &mut nodes);
    let leaves: Vec<usize> = (0..nodes.len())
        .filter(|&i| matches!(nodes[i].what(), Some(What::Leaf { .. })))
        .collect();
    if rng.one_in(2) {
        let hero = rng.pick(&leaves);
        nodes[hero] = nodes[hero].hero();
    }
    nodes
}

fn gaps(outer: Rect16, rects: &[Rect16], across: bool) -> Vec<i32> {
    let span = |r: Rect16| {
        if across {
            (i32::from(r.left), i32::from(r.right))
        } else {
            (i32::from(r.top), i32::from(r.bottom))
        }
    };
    let (start, end) = span(outer);
    let mut out = Vec::new();
    let mut edge = start;
    for &rect in rects {
        let (from, to) = span(rect);
        out.push(from - edge);
        edge = to;
    }
    out.push(end - edge);
    out
}

#[test]
fn slack_becomes_even_gaps_outer_edges_included_measured_ink_to_ink() {
    let (mut screens, mut checked) = (0, 0);
    for seed in 0..1500 {
        let mut rng = Rng::new(seed);
        let nodes = any_screen(&mut rng, 32);
        let end = ends(&nodes);
        for at in panels() {
            let Ok(screen) = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE) else {
                continue;
            };
            screens += 1;
            let mentioned = nodes
                .iter()
                .fold(Flags::NONE, |f, n| f.with(n.condition().mentions()));
            for flags in mentioned.subsets() {
                let mut seen = Seen::new();
                solve::solve(
                    screen.tree(),
                    whole(&at),
                    flags,
                    Balance::On,
                    None,
                    &mut seen,
                )
                .unwrap();
                for (i, node) in nodes.iter().enumerate() {
                    let across = match node.what().unwrap() {
                        What::Row { .. } => true,
                        What::Col { .. } => false,
                        _ => continue,
                    };
                    let Some(outer) = seen.rects[i] else {
                        continue;
                    };
                    let shown: Vec<usize> = children(&end, i)
                        .into_iter()
                        .filter(|&c| seen.rects[c].is_some())
                        .collect();
                    if shown.iter().any(|&c| nodes[c].size().weight() > 0) {
                        continue;
                    }
                    let rects: Vec<Rect16> =
                        shown.iter().map(|&c| seen.rects[c].unwrap()).collect();
                    let gaps = gaps(outer, &rects, across);
                    let (low, high) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
                    assert!(
                        high - low <= 1,
                        "seed {seed} on {:?} with {flags}: node {i}'s gaps {gaps:?}",
                        at.panel
                    );
                    for &c in &shown {
                        if nodes[c].size().get() != Sizing::Content {
                            continue;
                        }
                        let Some(placed) = seen
                            .solved
                            .leaves()
                            .iter()
                            .find(|p| usize::from(p.node) == c)
                        else {
                            continue;
                        };
                        let form = forms_of(nodes[c], &at)[usize::from(placed.form)];
                        let (ink, extent) = if across {
                            (form.width, placed.rect.width())
                        } else {
                            (form.height, placed.rect.height())
                        };
                        assert_eq!(ink, extent, "seed {seed}: node {c}'s look fills its box");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(
        screens > 1000 && checked > 5000,
        "{screens} screens, {checked} containers"
    );
}

#[test]
fn the_hero_upgrades_before_the_others_and_the_others_in_order() {
    let mut contested = 0;
    for seed in 0..2000 {
        let mut rng = Rng::new(seed);
        let metric = rng.pick(&Metric::ALL) as u8;
        let count = rng.range(2, 4) as usize;
        let across = rng.one_in(3);
        let hero = rng.option(|rng| rng.below(count as u64) as usize);
        let units = rng.range(0, 127) as u8;
        let mut nodes = vec![
            if across {
                Node::row(count as u8 + 1, Size::grow(1))
            } else {
                Node::col(count as u8 + 1, Size::grow(1))
            },
            tile(SPACE, 0, Size::units(units)),
        ];
        for i in 0..count {
            let leaf = tile(VALUE, metric, Size::CONTENT);
            nodes.push(if Some(i) == hero { leaf.hero() } else { leaf });
        }
        let at = rng.pick(&panels());
        let Ok(screen) = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE) else {
            continue;
        };
        let mut solved = Solved::EMPTY;
        screen.solve(Flags::NONE, &mut solved);
        let form = |i: usize| solved.leaves()[i + 1].form;
        let rank = |i: usize| (Some(i) != hero, i);
        if (0..count).any(|i| form(i) != form(0)) {
            contested += 1;
        }
        for a in 0..count {
            for b in 0..count {
                if rank(a) < rank(b) {
                    assert!(
                        form(a) <= form(b),
                        "seed {seed}: tile {a} ranks first but shows look {} to {b}'s {}",
                        form(a),
                        form(b)
                    );
                }
            }
        }
    }
    assert!(contested > 300, "{contested} trees where looks differ");
}

fn contest(first: Node, second: Node, room: u16) -> Vec<Placed> {
    let at = BASE;
    let filler = at.panel.height as u16 - room;
    let third = filler / 3;
    let nodes = [
        Node::col(5, Size::grow(1)),
        tile(SPACE, 0, Size::units(third as u8)),
        tile(SPACE, 0, Size::units(third as u8)),
        tile(SPACE, 0, Size::units((filler - 2 * third) as u8)),
        first,
        second,
    ];
    let screen = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).unwrap();
    let mut solved = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut solved);
    solved.leaves().to_vec()
}

fn look_of(leaves: &[Placed], node: u8) -> u8 {
    leaves.iter().find(|p| p.node == node).unwrap().form
}

#[test]
fn the_hero_reaches_its_best_look_where_preorder_would_spend_the_room_first() {
    let at = BASE;
    let speed = forms_of(tile(VALUE, Metric::Speed as u8, Size::CONTENT), &at);
    let power = forms_of(tile(VALUE, Metric::Power as u8, Size::CONTENT), &at);
    let room = speed[0].height + power.last().unwrap().height + 1;
    let (power_tile, speed_tile) = (
        tile(VALUE, Metric::Power as u8, Size::CONTENT),
        tile(VALUE, Metric::Speed as u8, Size::CONTENT),
    );
    let plain = contest(power_tile, speed_tile, room);
    assert!(
        look_of(&plain, 5) > 0,
        "first in preorder, power takes the room"
    );
    let hero = contest(power_tile, speed_tile.hero(), room);
    assert_eq!(look_of(&hero, 5), 0, "speed shows HERO");
    assert!(look_of(&hero, 4) > look_of(&plain, 4));
}

#[test]
fn the_heavier_tile_upgrades_before_the_lighter() {
    let at = BASE;
    let power = forms_of(tile(VALUE, Metric::Power as u8, Size::CONTENT), &at);
    let smallest = power.last().unwrap().height;
    let room = power[1].height + smallest + 1;
    let light = tile(VALUE, Metric::Power as u8, Size::grow(1));
    let heavy = tile(VALUE, Metric::Power as u8, Size::grow(2));
    let solved = contest(light, heavy, room);
    assert!(
        look_of(&solved, 5) < look_of(&solved, 4),
        "the heavier upgrades"
    );
    let even = contest(light, light, room);
    assert!(
        look_of(&even, 4) < look_of(&even, 5),
        "between equals, the first"
    );
}

#[test]
fn a_tile_alone_takes_the_whole_box_and_its_best_look() {
    let nodes = [tile(VALUE, Metric::Speed as u8, Size::CONTENT)];
    for at in panels() {
        let screen = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).unwrap();
        let mut solved = Solved::EMPTY;
        screen.solve(Flags::NONE, &mut solved);
        assert_eq!(solved.leaves()[0].rect, whole(&at));
        assert_eq!(solved.leaves()[0].form, 0);
    }
}

#[test]
fn tiles_in_a_row_share_the_baseline_of_their_largest_look() {
    for at in panels() {
        let screen = Screen::parse(0, &MAIN_BYTES, &at, Controls::FREE).unwrap();
        let end = ends(&MAIN);
        let mut solved = Solved::EMPTY;
        screen.solve(Flags::NONE, &mut solved);
        let mut canvas = Canvas::default();
        solved.prepare(&Nothing, &mut canvas);
        canvas.assert_kept("main");
        let mut rows = 0;
        for (i, node) in MAIN.iter().enumerate() {
            if !matches!(node.what(), Some(What::Row { .. })) {
                continue;
            }
            let leaves: Vec<&Placed> = children(&end, i)
                .into_iter()
                .filter_map(|c| solved.leaves().iter().find(|p| usize::from(p.node) == c))
                .collect();
            let Some(first) = leaves.first() else {
                continue;
            };
            let bottom = usize::from(first.rect.bottom);
            for placed in &leaves {
                let What::Leaf { component, arg } = MAIN[usize::from(placed.node)].known() else {
                    continue;
                };
                let mut own = Canvas::default();
                registry::tile(component).prepare(arg, placed, &at, &Nothing, &mut own);
                for run in &own.runs {
                    let face = run.line.face();
                    let baseline = run.top + face.fit_height();
                    assert_eq!(baseline, bottom, "row {i} on {:?}", at.panel);
                }
                for shape in &own.shapes {
                    assert_eq!(shape.bounds().bottom, bottom, "row {i} on {:?}", at.panel);
                }
            }
            rows += 1;
        }
        assert!(rows >= 4);
    }
}

fn any_record(rng: &mut Rng) -> Vec<u8> {
    let mut out = vec![VERSION];
    let screens = rng.below(4) as u8;
    out.push(screens);
    for key in 0..screens {
        let budget = rng.range(1, 32) as usize;
        let nodes = any_screen(rng, budget);
        out.push(key * 3 + rng.below(3) as u8);
        out.push(nodes.len() as u8);
        out.extend(bytes(&nodes));
    }
    let layers = rng.below(3) as u8;
    out.push(layers);
    for _ in 0..layers {
        let budget = rng.range(1, 6) as usize;
        let nodes = any_screen(rng, budget);
        let anchor = rng.below(7) as u8;
        let mode = u8::from(anchor < 2 && rng.one_in(2));
        out.extend([
            anchor << 4 | mode,
            rng.below(4) as u8,
            rng.byte(),
            nodes.len() as u8,
        ]);
        out.extend(bytes(&nodes));
    }
    out
}

#[test]
fn any_bytes_are_refused_or_encode_back_to_themselves() {
    let (mut accepted, mut refused) = (0, 0);
    let mut screens = Box::new(Screens::EMPTY);
    let mut out = vec![0; parse::RECORD];
    for seed in 0..12_000 {
        let mut rng = Rng::new(seed);
        let input = match rng.below(4) {
            0 => rng.bytes(80),
            1 => {
                let record = any_record(&mut rng);
                rng.mutate(record)
            }
            _ => any_record(&mut rng),
        };
        let at = rng.pick(&panels());
        match parse::parse(&input, &at, &no_cues, &mut screens) {
            Ok(()) => {
                let length = screens.encode(&mut out).unwrap();
                assert_eq!(&out[..length], &input[..], "seed {seed}");
                accepted += 1;
            }
            Err(reject) => {
                assert_eq!(*screens, Screens::EMPTY);
                assert!(!reject.to_string().is_empty());
                refused += 1;
            }
        }
        let nodes = rng.bytes(40);
        if let Ok(screen) = Screen::parse(1, &nodes, &at, Controls::FREE) {
            assert_eq!(bytes(screen.tree().nodes()), nodes, "seed {seed}");
        }
    }
    assert!(
        accepted > 2000 && refused > 2000,
        "{accepted} accepted, {refused} refused"
    );
}

#[test]
fn a_record_names_what_it_refuses() {
    let at = BASE;
    let refuse = |record: &[u8]| {
        let mut screens = Box::new(Screens::EMPTY);
        parse::parse(record, &at, &no_cues, &mut screens)
            .unwrap_err()
            .why
    };
    assert_eq!(refuse(&[]), Why::Truncated);
    assert_eq!(refuse(&[VERSION + 1, 0, 0]), Why::Version(VERSION + 1));
    assert_eq!(refuse(&[VERSION, 0, 0, 7]), Why::Trailing);
    let leaf = tile(SPACE, 0, Size::CONTENT).bytes();
    let twice = [&[VERSION, 2, 4, 1][..], &leaf, &[4, 1], &leaf, &[0]].concat();
    assert_eq!(refuse(&twice), Why::DuplicateScreen(4));
    let screen = |nodes: &[Node]| {
        Screen::parse(0, &bytes(nodes), &at, Controls::FREE)
            .unwrap_err()
            .why
    };
    let space = tile(SPACE, 0, Size::CONTENT);
    assert_eq!(
        screen(&[Node::col(2, Size::CONTENT), space]),
        Why::Shape { node: 0 }
    );
    assert_eq!(screen(&[space, space]), Why::Shape { node: 1 });
    assert_eq!(
        screen(&[Node::col(0, Size::CONTENT)]),
        Why::Empty { node: 0 }
    );
    assert_eq!(
        screen(&[tile(VALUE, 99, Size::CONTENT)]),
        Why::Arg { node: 0, arg: 99 }
    );
    assert_eq!(
        screen(&[tile(120, 0, Size::CONTENT)]),
        Why::Kind { node: 0, kind: 124 }
    );
    assert_eq!(
        screen(&[space.when(When::new(Flags::COURSE, Flags::NONE))]),
        Why::Root
    );
    let never = When::new(Flags::COURSE, Flags::COURSE);
    assert_eq!(
        screen(&[Node::col(1, Size::CONTENT), space.when(never)]),
        Why::Never { node: 1 }
    );
    assert_eq!(
        screen(&[Node::col(1, Size::CONTENT).hero(), space]),
        Why::Hero { node: 0 }
    );
    let two_heroes = [Node::col(2, Size::CONTENT), space.hero(), space.hero()];
    assert_eq!(screen(&two_heroes), Why::Hero { node: 2 });
    let mut deep = vec![Node::col(1, Size::CONTENT); 6];
    deep.push(space);
    assert_eq!(screen(&deep), Why::Depth { node: 6 });
    deep.remove(0);
    assert!(
        Screen::parse(0, &bytes(&deep), &at, Controls::FREE).is_ok(),
        "six levels"
    );
    let scrolls = [
        Node::col(2, Size::CONTENT),
        Node::scroll(ListKind::Rides, Size::grow(1)),
        space,
        Node::scroll(ListKind::Rides, Size::grow(1)),
        space,
    ];
    assert_eq!(screen(&scrolls), Why::Scroll { node: 3 });
    let many = vec![space; 25];
    let wide = [&[Node::row(25, Size::CONTENT)][..], &many].concat();
    assert_eq!(
        screen(&wide),
        Why::Leaves {
            count: 25,
            flags: Flags::NONE
        }
    );
    let gauges = [
        Node::col(4, Size::CONTENT),
        tile(GAUGE, 0, Size::CONTENT),
        tile(GAUGE, 0, Size::CONTENT),
        tile(GAUGE, 0, Size::CONTENT),
        tile(GAUGE, 0, Size::CONTENT),
    ];
    assert_eq!(
        screen(&gauges),
        Why::Canvas {
            shapes: 28,
            runs: 0,
            flags: Flags::NONE
        }
    );
}

#[test]
fn a_tree_of_too_many_tiles_is_refused_before_one_is_placed() {
    let at = BASE;
    let many = [
        &[Node::row(25, Size::CONTENT)][..],
        &[tile(SPACE, 0, Size::CONTENT); 25],
    ]
    .concat();
    let tree = parse::tree(&bytes(&many), &at).unwrap();
    let mut seen = Seen::new();
    let result = solve::solve(&tree, whole(&at), Flags::NONE, Balance::On, None, &mut seen);
    assert_eq!(
        result.map(|_| ()),
        Err(Why::Leaves {
            count: 25,
            flags: Flags::NONE
        })
    );
    assert_eq!(seen.solved.leaves(), []);
}

#[test]
fn accept_refuses_a_tile_squeezed_under_one_condition_with_the_node_and_pixels() {
    let at = BASE;
    let course = When::new(Flags::COURSE, Flags::NONE);
    let nodes = [
        Node::col(5, Size::grow(1)),
        tile(SPACE, 0, Size::units(22)),
        tile(SPACE, 0, Size::units(127)).when(course),
        tile(SPACE, 0, Size::units(127)).when(course),
        tile(SPACE, 0, Size::units(40)).when(course),
        tile(VALUE, Metric::Speed as u8, Size::CONTENT),
    ];
    let needs = at.face(FaceRole::Body).fit_height() as u16;
    let reject = Screen::parse(7, &bytes(&nodes), &at, Controls::FREE).unwrap_err();
    assert_eq!(
        reject,
        Reject {
            place: Place::Screen(7),
            why: Why::Misfit(Misfit {
                node: 5,
                what: nodes[5],
                axis: Axis::Down,
                gets: 320 - 316,
                needs,
                flags: Flags::COURSE,
            }),
        }
    );
    assert_eq!(
        reject.to_string(),
        format!("screen 7: node 5 value(SPEED) gets 4 px tall, needs {needs} (with COURSE)")
    );
    let without = [Node::col(2, Size::grow(1)), nodes[1], nodes[5]];
    assert!(Screen::parse(7, &bytes(&without), &at, Controls::FREE).is_ok());
}

#[test]
fn a_band_too_wide_for_its_tiles_is_refused_across() {
    let at = Scaled::of(180, 240);
    let nodes = [
        Node::row(3, Size::grow(1)),
        tile(SPACE, 0, Size::units(127)),
        tile(SPACE, 0, Size::units(30)),
        tile(GAUGE, Effort::Power as u8, Size::CONTENT),
    ];
    let reject = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).unwrap_err();
    let Why::Misfit(misfit) = reject.why else {
        panic!("{reject}");
    };
    let smallest = forms_of(nodes[3], &at).last().unwrap().width;
    assert_eq!((misfit.node, misfit.axis), (3, Axis::Across));
    assert_eq!(misfit.gets, 180 - at.unit(157) as u16);
    assert_eq!(misfit.needs, smallest);
}

#[test]
fn a_stack_shows_its_first_present_child_and_fits_every_child() {
    let at = BASE;
    let course = When::new(Flags::COURSE, Flags::NONE);
    let nodes = [
        Node::stack(2, Size::grow(1)),
        tile(VALUE, Metric::Speed as u8, Size::CONTENT).when(course),
        tile(CAPTION, Metric::Power as u8, Size::CONTENT),
    ];
    let screen = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).unwrap();
    let mut solved = Solved::EMPTY;
    screen.solve(Flags::COURSE, &mut solved);
    assert_eq!(
        solved.leaves().iter().map(|p| p.node).collect::<Vec<_>>(),
        [1]
    );
    screen.solve(Flags::NONE, &mut solved);
    assert_eq!(
        solved.leaves().iter().map(|p| p.node).collect::<Vec<_>>(),
        [2]
    );
    let hidden_misfit = [
        Node::col(2, Size::grow(1)),
        tile(SPACE, 0, Size::units(127)),
        Node::stack(2, Size::units(4)),
        tile(SPACE, 0, Size::CONTENT),
        tile(VALUE, Metric::Speed as u8, Size::CONTENT),
    ];
    let reject = Screen::parse(0, &bytes(&hidden_misfit), &at, Controls::FREE).unwrap_err();
    assert!(
        matches!(reject.why, Why::Misfit(Misfit { node: 4, .. })),
        "{reject}"
    );
}

#[test]
fn a_scroll_places_one_item_of_its_template_one_gap_under_its_top_a_gutter_in() {
    let at = BASE;
    let nodes = [
        Node::col(2, Size::grow(1)),
        tile(SPACE, 0, Size::units(22)),
        Node::scroll(ListKind::Workouts, Size::grow(1)),
        Node::row(2, Size::units(40)),
        tile(CAPTION, Metric::Distance as u8, Size::CONTENT),
        tile(VALUE, Metric::Distance as u8, Size::CONTENT),
    ];
    let screen = Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).unwrap();
    let mut solved = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut solved);
    let (scroll, template) = solved.scroll().unwrap();
    let viewport = Rect16 {
        left: 0,
        top: 22,
        right: 240,
        bottom: 320,
    };
    let (gap, gutter) = (list::gap(&at), list::gutter(&at));
    assert_eq!(
        (scroll.node, scroll.viewport, scroll.template),
        (
            2,
            viewport,
            Rect16 {
                left: gutter,
                top: 22 + gap,
                right: 240 - gutter,
                bottom: 22 + gap + 40,
            }
        )
    );
    assert_eq!(template, &solved.leaves()[1..]);
    assert_eq!(template.len(), 2);
    for placed in template {
        assert_eq!(placed.rect.bottom, 22 + gap + 40);
    }
}

#[test]
fn a_push_layer_takes_a_band_and_the_screen_retiles_under_it() {
    let at = BASE;
    let screen = [tile(VALUE, Metric::Speed as u8, Size::CONTENT)];
    let popover = [tile(CAPTION, Metric::Grade as u8, Size::CONTENT)];
    let record = [
        &[VERSION, 1, 3, 1][..],
        &bytes(&screen),
        &[1, 1, 1, 5, 1],
        &bytes(&popover),
    ]
    .concat();
    let mut screens = Box::new(Screens::EMPTY);
    parse::parse(&record, &at, &no_cues, &mut screens).unwrap();
    let layer: &Layer = &screens.layers()[0];
    assert_eq!(
        (layer.anchor(), layer.hold()),
        (Anchor::Top, Hold::Seconds(NonZeroU8::new(5).unwrap()))
    );
    let band = at.face(FaceRole::Caption).fit_height() as u16;
    let mut solved = Solved::EMPTY;
    layer.solve(Flags::NONE, &mut solved);
    assert_eq!(
        solved.leaves()[0].rect,
        Rect16 {
            left: 0,
            top: 0,
            right: 240,
            bottom: band
        }
    );
    assert!(screens.solve(3, Flags::NONE, Some(0), &mut solved));
    assert_eq!(
        solved.leaves()[0].rect,
        Rect16 {
            left: 0,
            top: band,
            right: 240,
            bottom: 320
        }
    );
    assert!(screens.solve(3, Flags::NONE, None, &mut solved));
    assert_eq!(solved.leaves()[0].rect, whole(&at));
    assert!(!screens.solve(4, Flags::NONE, None, &mut solved));
    let centred = [&[VERSION, 0, 1, 2 << 4 | 1, 0, 0, 1][..], &bytes(&popover)].concat();
    assert_eq!(
        parse::parse(&centred, &at, &no_cues, &mut screens)
            .unwrap_err()
            .why,
        Why::Push
    );
}

fn play_and_back() -> Controls {
    let buttons = Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: Some(Icon::Play),
        }),
        back: Some(Button {
            action: Action::Page(OnPage::Back),
            cue: Some(Icon::Back),
        }),
        move_: None,
        held: None,
    };
    Controls::new(buttons, crate::screens::C606.keys)
}

#[test]
fn a_tile_is_accepted_clear_of_its_screens_cues_or_refused_at_accept() {
    let at = BASE;
    let controls = play_and_back();
    let cues = controls.cues().unwrap().clone();
    let (mut cut, mut drawn, mut refused) = (0, 0, 0);
    let filler = tile(SPACE, 0, Size::grow(1));
    for (component, kind) in registry::every() {
        for arg in (0..=255).filter(|&arg| kind.accepts(arg)) {
            for width in (4..=127).step_by(3) {
                let leaf = tile(component, arg, Size::units(width));
                for band in [[leaf, filler], [filler, leaf]] {
                    let nodes = [
                        Node::col(3, Size::grow(1)),
                        tile(SPACE, 0, Size::units(60)),
                        Node::row(2, Size::CONTENT),
                        band[0],
                        band[1],
                        filler,
                    ];
                    let node = if band[0] == leaf { 3 } else { 4 };
                    if Screen::parse(0, &bytes(&nodes), &at, Controls::FREE).is_err() {
                        continue;
                    }
                    let screen = match Screen::parse(0, &bytes(&nodes), &at, controls.clone()) {
                        Ok(screen) => screen,
                        Err(Reject {
                            why: Why::Misfit(misfit),
                            ..
                        }) => {
                            assert_eq!(usize::from(misfit.node), node);
                            assert!(misfit.gets < misfit.needs, "{misfit:?}");
                            refused += 1;
                            continue;
                        }
                        Err(other) => panic!("{other}"),
                    };
                    let mut solved = Solved::EMPTY;
                    screen.solve(Flags::NONE, &mut solved);
                    let mut seen = Seen::new();
                    let solve = solve::solve(
                        screen.tree(),
                        whole(&at),
                        Flags::NONE,
                        Balance::On,
                        Some(&cues),
                        &mut seen,
                    );
                    assert!(solve.is_ok());
                    let device = seen.rects[node].unwrap();
                    let keys = &crate::screens::C606.keys;
                    let clear = cues
                        .shown()
                        .iter()
                        .fold(device.rect(), |rect, cue| cue.clear(keys, &at, rect));
                    let mut canvas = Canvas::default();
                    solved.prepare(&Nothing, &mut canvas);
                    let shapes = canvas.shapes.iter().map(|shape| shape.bounds());
                    let runs = canvas.runs.iter().map(|run| Rect {
                        left: run.left,
                        top: run.top,
                        right: run.left + run.line.width() + SPILL,
                        bottom: run.bottom(),
                    });
                    for drawn_at in shapes.chain(runs) {
                        assert!(
                            !cues
                                .shown()
                                .iter()
                                .any(|cue| cue.covers(drawn_at, &crate::screens::C606)),
                            "{}({arg}) {width} units wide draws {drawn_at:?} on a tab",
                            kind.name(),
                        );
                        drawn += 1;
                    }
                    if clear != device.rect() {
                        cut += 1;
                    }
                }
            }
        }
    }
    assert!(
        cut > 100 && drawn > 1000 && refused > 0,
        "{cut} cut, {drawn} drawn, {refused} refused"
    );
}

#[test]
fn a_tile_too_wide_beside_a_cue_is_refused_by_the_screen_that_shows_it() {
    let at = BASE;
    let smallest = forms_of(tile(VALUE, Metric::Elapsed as u8, Size::CONTENT), &at)
        .last()
        .unwrap()
        .width;
    let filler = tile(SPACE, 0, Size::grow(1));
    let time = tile(VALUE, Metric::Elapsed as u8, Size::units(smallest as u8));
    let nodes = [
        Node::col(3, Size::grow(1)),
        tile(SPACE, 0, Size::units(60)),
        Node::row(2, Size::CONTENT),
        time,
        filler,
        filler,
    ];
    let screen = Screen::parse(7, &bytes(&nodes), &at, Controls::FREE).unwrap();
    let mut solved = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut solved);
    let reject = Screen::parse(7, &bytes(&nodes), &at, play_and_back()).unwrap_err();
    let Why::Misfit(misfit) = reject.why else {
        panic!("{reject}");
    };
    assert_eq!(
        (reject.place, misfit.node, misfit.axis, misfit.needs),
        (Place::Screen(7), 3, Axis::Across, smallest)
    );
    assert!(misfit.gets < misfit.needs, "{misfit:?}");
    let record = [
        &[VERSION, 1, 7, nodes.len() as u8][..],
        &bytes(&nodes),
        &[0],
    ]
    .concat();
    let mut screens = Box::new(Screens::EMPTY);
    let only_seven = |key| {
        if key == 7 {
            play_and_back()
        } else {
            Controls::FREE
        }
    };
    assert_eq!(
        parse::parse(&record, &at, &only_seven, &mut screens),
        Err(reject)
    );
    assert!(parse::parse(&record, &at, &no_cues, &mut screens).is_ok());
}

#[test]
fn every_tile_offers_looks_that_shrink_and_fit_the_canvas() {
    assert_eq!(Canvas::default().shapes.capacity(), usize::from(MAX_SHAPES));
    assert_eq!(Canvas::default().runs.capacity(), usize::from(MAX_RUNS));
    let names: Vec<&str> = registry::every().map(|(_, tile)| tile.name()).collect();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "a catalogue names each tile once: {names:?}"
    );
    assert_eq!(names[..3], ["space", "value", "caption"]);
    assert_eq!(registry::tile(registry::MAP).name(), "map");
    assert_eq!(registry::tile(registry::SCALE).name(), "scale");
    for at in panels() {
        // A script's looks are its module's, checked at accept (script/tests.rs).
        for (_, tile) in registry::every().filter(|(_, tile)| tile.name() != "script") {
            for arg in (0..=255).filter(|&arg| tile.accepts(arg)) {
                let forms = tile.forms(arg, &at);
                assert!(!forms.is_empty(), "{}({arg})", tile.name());
                for pair in forms.windows(2) {
                    assert!(
                        pair[1].width <= pair[0].width && pair[1].height <= pair[0].height,
                        "{}({arg}): {pair:?}",
                        tile.name()
                    );
                }
            }
        }
    }
    assert!(!registry::tile(VALUE).accepts(Metric::ALL.len() as u8));
    assert!(registry::tile(ICON).accepts(Glyph::Puck as u8));
}

struct Reading(Option<i32>);

impl Data for Reading {
    fn metric(&self, _: Metric) -> Option<i32> {
        self.0
    }
    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }
    fn list(&self, _: ListKind) -> u16 {
        0
    }
}

#[test]
fn every_reading_shows_whole_in_its_metrics_smallest_look() {
    let edges = [
        None,
        Some(i32::MIN),
        Some(-1),
        Some(0),
        Some(i32::MAX),
        Some(27_778),
        Some(1_000_000),
        Some(10_000_000),
        Some(36_000),
        Some(360_000),
        Some(2000),
        Some(10_000),
        Some(-300),
        Some(-1000),
    ];
    let mut rng = Rng::new(7);
    let mut checked = 0;
    for at in panels() {
        for metric in Metric::ALL {
            let forms = registry::tile(VALUE).forms(metric as u8, &at);
            let smallest = forms.len() - 1;
            let look = forms[smallest];
            let placed = Placed {
                rect: Rect16 {
                    left: 4,
                    top: 4,
                    right: 4 + look.width,
                    bottom: 4 + look.height,
                },
                node: 0,
                form: smallest as u8,
            };
            let random: Vec<_> = (0..600)
                .map(|n| match n % 2 {
                    0 => Some(rng.below(1 << 32) as u32 as i32),
                    _ => Some(rng.below(400_000) as i32 - 2000),
                })
                .collect();
            for value in edges.into_iter().chain(random) {
                let mut text = Text::<12>::new();
                metric.write(value, &mut text);
                let mut canvas = Canvas::default();
                registry::tile(VALUE).prepare(
                    metric as u8,
                    &placed,
                    &at,
                    &Reading(value),
                    &mut canvas,
                );
                let run = canvas.runs[0];
                assert_eq!(
                    run.line.text(),
                    text.as_bytes(),
                    "{metric:?} {value:?} on {:?}",
                    at.panel
                );
                assert!(
                    run.left >= usize::from(placed.rect.left)
                        && run.left + run.line.width() + SPILL <= usize::from(placed.rect.right),
                    "{metric:?} {value:?} leaves its look on {:?}",
                    at.panel
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 5000, "{checked} readings");
}

#[test]
fn a_reading_past_its_metrics_ends_shows_as_the_end() {
    let shows = |metric: Metric, value: i32| {
        let mut text = Text::<12>::new();
        metric.write(Some(value), &mut text);
        text.as_str().to_string()
    };
    assert_eq!(shows(Metric::Elapsed, 36_000), "10:00:00");
    assert_eq!(shows(Metric::Elapsed, i32::MAX), "99:59:59");
    assert_eq!(shows(Metric::Distance, 1_234_560), "1234.5");
    assert_eq!(shows(Metric::Distance, i32::MAX), "9999.9");
    assert_eq!(shows(Metric::Speed, 27_777), "99.9");
    assert_eq!(shows(Metric::Speed, 27_778), "100");
    assert_eq!(shows(Metric::Speed, 33_334), "120");
    assert_eq!(shows(Metric::Speed, 40_000), "144");
    assert_eq!(shows(Metric::Speed, i32::MAX), "144");
    assert_eq!(shows(Metric::Speed, -5_000), "0.0");
    assert_eq!(shows(Metric::Power, 2_400), "2400");
    assert_eq!(shows(Metric::Grade, i32::MIN), "-99.9%");
    assert_eq!(shows(Metric::Kcal, i32::MAX), "999K");
}

/// The widest text of each tile: every reading past its far end.
struct Widest;

impl Data for Widest {
    fn metric(&self, metric: Metric) -> Option<i32> {
        Some(match metric {
            Metric::Grade => i32::MIN,
            _ => i32::MAX,
        })
    }
    fn zone(&self, _: Effort) -> Option<u8> {
        Some(7)
    }
    fn list(&self, _: ListKind) -> u16 {
        0
    }
}

#[test]
fn a_text_tile_in_a_box_exactly_its_look_shows_its_whole_text() {
    let mut checked = 0;
    for at in panels() {
        for component in [VALUE, CAPTION, UNIT, ZONE, READING, TOTAL, EFFORT] {
            let tile = registry::tile(component);
            for arg in (0..=255).filter(|&arg| tile.accepts(arg)) {
                for (form, look) in tile.forms(arg, &at).iter().enumerate() {
                    let draw = |width: u16| {
                        let rect = Rect16 {
                            left: 4,
                            top: 4,
                            right: 4 + width,
                            bottom: 4 + look.height,
                        };
                        let placed = Placed {
                            rect,
                            node: 0,
                            form: form as u8,
                        };
                        let mut canvas = Canvas::default();
                        tile.prepare(arg, &placed, &at, &Widest, &mut canvas);
                        let texts: Vec<Vec<u8>> = canvas
                            .runs
                            .iter()
                            .map(|run| run.line.text().to_vec())
                            .collect();
                        texts
                    };
                    let (exact, roomy) = (draw(look.width), draw(look.width * 2 + 8));
                    assert_eq!(
                        exact,
                        roomy,
                        "{}({arg}) look {form} on {:?}",
                        tile.name(),
                        at.panel
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 40, "{checked} looks");
}

#[test]
fn a_layout_draws_the_tiles_of_the_screen_it_was_last_solved_for() {
    let at = BASE;
    let main = Screen::parse(0, &MAIN_BYTES, &at, Controls::FREE).unwrap();
    let one = [tile(VALUE, Metric::Speed as u8, Size::CONTENT)];
    let one = Screen::parse(1, &bytes(&one), &at, Controls::FREE).unwrap();
    let mut solved = Solved::EMPTY;
    let mut canvas = Canvas::default();
    solved.prepare(&Nothing, &mut canvas);
    assert_eq!((canvas.runs.len(), canvas.shapes.len()), (0, 0));
    main.solve(Flags::NONE, &mut solved);
    one.solve(Flags::NONE, &mut solved);
    solved.prepare(&Nothing, &mut canvas);
    assert_eq!((canvas.runs.len(), canvas.shapes.len()), (1, 0));
}

#[test]
fn the_main_page_tree_is_accepted_on_every_panel_and_times_its_solve() {
    let started = std::time::Instant::now();
    let now = || started.elapsed().as_micros() as u64;
    for at in panels() {
        let timing = super::bench::measure(&at, &now).unwrap();
        assert_eq!(
            timing.leaves, 13,
            "the course chip shows, the lower line does not"
        );
        std::println!("compose {:?}: {timing:?}", at.panel);
    }
}

#[test]
fn a_text_tile_in_a_top_layer_draws_inside_its_band() {
    let mut drawn = 0;
    for at in panels() {
        for component in [VALUE, CAPTION, UNIT, ZONE] {
            let kind = registry::tile(component);
            for arg in (0..=255).filter(|&arg| kind.accepts(arg)) {
                let popover = [tile(component, arg, Size::CONTENT)];
                let record = [&[VERSION, 0, 1, 0, 0, 5, 1][..], &bytes(&popover)].concat();
                let mut screens = Box::new(Screens::EMPTY);
                parse::parse(&record, &at, &no_cues, &mut screens).unwrap();
                let mut solved = Solved::EMPTY;
                screens.layers()[0].solve(Flags::NONE, &mut solved);
                let band = solved.leaves()[0].rect;
                assert_eq!(band.top, 0);
                let mut canvas = Canvas::default();
                solved.prepare(&Widest, &mut canvas);
                let run = canvas.runs[0];
                assert_eq!(
                    run.top + run.line.face().fit_height(),
                    usize::from(band.bottom),
                    "{}({arg}) on {:?}",
                    kind.name(),
                    at.panel
                );
                drawn += 1;
            }
        }
    }
    assert!(drawn > 20, "{drawn} drawn");
}

#[test]
fn a_stack_costs_the_canvas_what_its_costliest_slide_does() {
    let at = BASE;
    let gauge = tile(GAUGE, Effort::Power as u8, Size::CONTENT);
    let slide = Node::col(2, Size::CONTENT);
    let nodes = [
        Node::stack(2, Size::grow(1)),
        slide,
        gauge,
        gauge,
        slide,
        gauge,
        gauge,
    ];
    let screen = Screen::parse(7, &bytes(&nodes), &at, Controls::FREE).unwrap();
    let mut solved = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut solved);
    let mut canvas = Canvas::default();
    solved.prepare(&Nothing, &mut canvas);
    assert_eq!(canvas.shapes.len(), 14);
    let beside = [
        Node::row(2, Size::grow(1)),
        nodes[0],
        nodes[1],
        gauge,
        gauge,
        nodes[4],
        gauge,
        gauge,
        Node::col(2, Size::CONTENT),
        gauge,
        gauge,
    ];
    assert_eq!(
        Screen::parse(7, &bytes(&beside), &at, Controls::FREE)
            .unwrap_err()
            .why,
        Why::Canvas {
            shapes: 28,
            runs: 0,
            flags: Flags::NONE
        }
    );
}

#[test]
fn a_screen_and_a_layer_over_it_are_refused_when_together_they_overflow_the_canvas() {
    let at = BASE;
    let caption = tile(CAPTION, Metric::Heart as u8, Size::CONTENT);
    let tree = |count: usize| {
        let mut nodes = vec![Node::col(count as u8, Size::grow(1))];
        nodes.extend(std::iter::repeat_n(caption, count));
        bytes(&nodes)
    };
    let record = |screen: usize, layer: usize| {
        [
            &[VERSION, 1, 7, screen as u8 + 1][..],
            &tree(screen),
            &[1, 0x10, 0, 5, layer as u8 + 1],
            &tree(layer),
        ]
        .concat()
    };
    let mut screens = Box::new(Screens::EMPTY);
    assert!(parse::parse(&record(14, 14), &at, &no_cues, &mut screens).is_ok());
    let mut page = Solved::EMPTY;
    assert!(screens.solve(7, Flags::NONE, None, &mut page));
    let mut layer = Solved::EMPTY;
    screens.layers()[0].solve(Flags::NONE, &mut layer);
    let mut canvas = Canvas::default();
    page.prepare(&Nothing, &mut canvas);
    layer.prepare(&Nothing, &mut canvas);
    canvas.assert_kept("14 captions under 14");
    let reject = parse::parse(&record(16, 16), &at, &no_cues, &mut screens).unwrap_err();
    assert_eq!(
        reject,
        Reject {
            place: Place::Screen(7),
            why: Why::Beneath {
                layer: 0,
                shapes: 0,
                runs: 32,
                flags: Flags::NONE
            },
        }
    );
    assert_eq!(
        reject.to_string(),
        "screen 7: with layer 0 over it the looks need 0 shapes and 32 runs"
    );
}

/// Every reading at one of the places its range turns: none, its low end,
/// its high end, or the last reading before it changes format. The other
/// queries answer at the matching end, so the high side is each one's
/// widest.
#[derive(Clone, Copy, Debug)]
enum Reach {
    Nothing,
    Low,
    Turn,
    High,
}

impl Data for Reach {
    fn metric(&self, metric: Metric) -> Option<i32> {
        let (low, high) = metric.ends();
        match self {
            Self::Nothing => None,
            Self::Low => Some(low),
            Self::Turn => Some(metric.turn().unwrap_or(high)),
            Self::High => Some(high),
        }
    }
    fn zone(&self, effort: Effort) -> Option<u8> {
        match self {
            Self::Nothing => None,
            Self::Low => Some(1),
            Self::Turn | Self::High => Some(effort.zones()),
        }
    }
    fn list(&self, _: ListKind) -> u16 {
        match self {
            Self::Nothing | Self::Low => 0,
            Self::Turn | Self::High => u16::MAX,
        }
    }
    fn gauge(&self, effort: Effort) -> Option<OnGauge> {
        let range = 100;
        match self {
            Self::Nothing => None,
            Self::Low => Some(OnGauge {
                zone: 1,
                within: 0,
                range,
            }),
            Self::Turn | Self::High => Some(OnGauge {
                zone: effort.zones(),
                within: range,
                range,
            }),
        }
    }
    fn target(&self, effort: Effort) -> Option<[OnGauge; 2]> {
        let from = self.gauge(effort)?;
        let low = OnGauge {
            zone: 1,
            within: 0,
            range: from.range,
        };
        Some([low, from])
    }
}

/// Every tile of every family, for every arg it accepts and in every one of
/// its looks, drawn in a box exactly that look on every panel with no
/// readings and with every reading at each of its turns: it draws all of
/// its text (as in a box twice as wide), inside the box, with no two lines
/// of text over each other. A look that understates its tile fails here.
#[test]
fn every_tile_draws_inside_each_look_at_every_reading_s_reach() {
    let mut checked = 0;
    for at in panels() {
        // A script's looks are its module's; accept's trial run refuses one
        // that draws past its box (script/tests.rs).
        for (_, tile) in registry::every().filter(|(_, tile)| tile.name() != "script") {
            for arg in (0..=255).filter(|&arg| tile.accepts(arg)) {
                for (form, look) in tile.forms(arg, &at).iter().enumerate() {
                    for reach in [Reach::Nothing, Reach::Low, Reach::Turn, Reach::High] {
                        let what = format!(
                            "{}({arg}) look {form} {look:?} at {reach:?} on {:?}",
                            tile.name(),
                            at.panel
                        );
                        let draw = |width: u16| {
                            let placed = Placed {
                                rect: Rect16 {
                                    left: 4,
                                    top: 4,
                                    right: 4 + width,
                                    bottom: 4 + look.height,
                                },
                                node: 0,
                                form: form as u8,
                            };
                            let mut canvas = Canvas::default();
                            tile.prepare(arg, &placed, &at, &reach, &mut canvas);
                            canvas
                        };
                        let exact = draw(look.width);
                        let roomy = draw(look.width * 2 + 8);
                        let texts = |canvas: &Canvas| -> Vec<Vec<u8>> {
                            canvas
                                .runs
                                .iter()
                                .map(|run| run.line.text().to_vec())
                                .collect()
                        };
                        assert_eq!(texts(&exact), texts(&roomy), "{what}: text is cut");
                        let inside = Rect {
                            left: 4,
                            top: 4,
                            right: 4 + usize::from(look.width),
                            bottom: 4 + usize::from(look.height),
                        };
                        let lines: Vec<Rect> = exact
                            .runs
                            .iter()
                            .filter(|run| !run.line.text().is_empty())
                            .map(|run| Rect {
                                left: run.left,
                                top: run.top,
                                right: run.left + run.line.width() + SPILL,
                                bottom: run.top + run.line.face().fit_height(),
                            })
                            .collect();
                        let shapes = exact.shapes.iter().map(|shape| shape.bounds());
                        for drawn in lines.iter().copied().chain(shapes) {
                            assert!(
                                drawn.left >= inside.left
                                    && drawn.top >= inside.top
                                    && drawn.right <= inside.right
                                    && drawn.bottom <= inside.bottom,
                                "{what}: {drawn:?} leaves {inside:?}"
                            );
                        }
                        // Lines meet on their ink: a unit follows its value's
                        // last glyph, not the spill past its advance.
                        let inks: Vec<Rect> = exact
                            .runs
                            .iter()
                            .filter(|run| !run.line.text().is_empty())
                            .map(|run| Rect {
                                left: run.left,
                                top: run.top,
                                right: run.left + run.line.face().ink_width(run.line.text()),
                                bottom: run.top + run.line.face().fit_height(),
                            })
                            .collect();
                        for (i, a) in inks.iter().enumerate() {
                            for b in &inks[i + 1..] {
                                let apart = a.right <= b.left
                                    || b.right <= a.left
                                    || a.bottom <= b.top
                                    || b.bottom <= a.top;
                                assert!(apart, "{what}: {a:?} over {b:?}");
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 1000, "{checked} drawn");
}

/// Every number a static screen reads, the same for each: a world at one of
/// the far ends a reading can stand at, or at one a word switches on.
/// None stands for no reading at all.
struct Everywhere(Option<i32>);

impl Data for Everywhere {
    fn metric(&self, _: Metric) -> Option<i32> {
        self.0
    }
    fn zone(&self, _: Effort) -> Option<u8> {
        self.0
            .and_then(|value| u8::try_from(value.clamp(1, 7)).ok())
    }
    fn list(&self, _: ListKind) -> u16 {
        0
    }
    fn count(&self, _: super::data::Count) -> Option<i32> {
        self.0
    }
}

const EVERYWHERE: [i32; 20] = [
    i32::MIN,
    -32_768,
    -9_999,
    -301,
    -31,
    -1,
    0,
    1,
    2,
    3,
    4,
    5,
    26,
    99,
    100,
    255,
    9_999,
    32_767,
    65_535,
    i32::MAX,
];

/// What `exact` draws in `rect` against the same tile drawn roomy: every
/// letter of the roomy drawing (a line may wrap), ink only inside `rect`,
/// and no two lines on each other's ink.
fn drawn_whole_inside(
    what: &str,
    rect: Rect,
    exact: &Canvas,
    roomy: &Canvas,
    faults: &mut Vec<std::string::String>,
) {
    use crate::screens::chrome::{Look, Shown};
    let letters = |canvas: &Canvas| -> Vec<u8> {
        canvas
            .runs
            .iter()
            .flat_map(|run| run.line.text().iter().copied())
            .filter(|&byte| byte != b' ')
            .collect()
    };
    if letters(exact) != letters(roomy) {
        faults.push(format!(
            "{what}: a text is cut: {:?}",
            exact
                .runs
                .iter()
                .map(|run| core::str::from_utf8(run.line.text()).unwrap_or("?"))
                .collect::<Vec<_>>()
        ));
    }
    for run in &exact.runs {
        for y in run.top..run.bottom() {
            for x in run.columns() {
                let inside = rect.left <= x && x < rect.right && rect.top <= y && y < rect.bottom;
                if !inside && run.pixel(Look::PLAIN, x, y, Shown(0)).is_some() {
                    faults.push(format!(
                        "{what}: {:?} inks ({x}, {y}) outside {rect:?}",
                        core::str::from_utf8(run.line.text()),
                    ));
                }
            }
        }
    }
    for (index, one) in exact.runs.iter().enumerate() {
        for other in &exact.runs[index + 1..] {
            let rows = one.top.max(other.top)..one.bottom().min(other.bottom());
            let columns = one.columns().start.max(other.columns().start)
                ..one.columns().end.min(other.columns().end);
            let shared = rows.flat_map(|y| columns.clone().map(move |x| (x, y)));
            let both = shared
                .filter(|&(x, y)| {
                    one.pixel(Look::PLAIN, x, y, Shown(0)).is_some()
                        && other.pixel(Look::PLAIN, x, y, Shown(0)).is_some()
                })
                .count();
            if both > 0 {
                faults.push(format!(
                    "{what}: {:?} and {:?} share ink",
                    core::str::from_utf8(one.line.text()),
                    core::str::from_utf8(other.line.text()),
                ));
            }
        }
    }
    for &shape in &exact.shapes {
        let bounds = shape.bounds();
        let mut alone = Canvas::default();
        alone.shape(shape);
        for y in bounds.top..bounds.bottom {
            for x in bounds.left..bounds.right {
                let inside = rect.left <= x && x < rect.right && rect.top <= y && y < rect.bottom;
                let inked = [Shown(0), Shown(0xffff)]
                    .into_iter()
                    .any(|under| alone.pixel(Look::PLAIN, x, y, under) != under);
                if !inside && inked {
                    faults.push(format!("{what}: a shape inks ({x}, {y}) outside {rect:?}"));
                }
            }
        }
    }
}

#[test]
fn a_static_tile_at_its_smallest_box_shows_every_reading_whole_and_apart() {
    let (mut checked, mut faults) = (0, Vec::new());
    for at in panels() {
        for component in [
            registry::LINE,
            registry::FTP,
            registry::BEST,
            registry::FORM_BAND,
            registry::TREND,
            registry::POWER_RING,
        ] {
            let tile = registry::tile(component);
            for arg in (0..=255).filter(|&arg| tile.accepts(arg)) {
                for (form, look) in tile.forms(arg, &at).iter().enumerate() {
                    let draw = |width: u16, data: &dyn Data| {
                        let rect = Rect16 {
                            left: 4,
                            top: 4,
                            right: 4 + width,
                            bottom: 4 + look.height,
                        };
                        let placed = Placed {
                            rect,
                            node: 0,
                            form: form as u8,
                        };
                        let mut canvas = Canvas::default();
                        tile.prepare(arg, &placed, &at, data, &mut canvas);
                        (rect.rect(), canvas)
                    };
                    let worlds = EVERYWHERE.iter().map(|&value| Everywhere(Some(value)));
                    for world in core::iter::once(Everywhere(None)).chain(worlds) {
                        let data = &world as &dyn Data;
                        let what = format!(
                            "{}({arg}) look {form} on {:?} at {:?}",
                            tile.name(),
                            at.panel,
                            world.0
                        );
                        let (rect, exact) = draw(look.width, data);
                        let (_, roomy) = draw(look.width * 2 + 8, data);
                        drawn_whole_inside(&what, rect, &exact, &roomy, &mut faults);
                        checked += 1;
                    }
                }
            }
        }
    }
    faults.dedup();
    assert!(
        faults.is_empty(),
        "{} faults:\n{}",
        faults.len(),
        faults[..faults.len().min(40)].join("\n")
    );
    assert!(checked > 100, "{checked} draws");
}

#[test]
fn a_best_narrower_than_its_watts_a_kilo_is_refused() {
    let nodes = [
        Node::row(3, Size::grow(1)),
        tile(SPACE, 0, Size::units(70)),
        tile(registry::BEST, 0, Size::units(70)),
        tile(SPACE, 0, Size::grow(1)),
    ];
    let reject = Screen::parse(1, &bytes(&nodes), &BASE, Controls::FREE).unwrap_err();
    assert!(
        matches!(reject.why, Why::Misfit(Misfit { node: 2, .. })),
        "{reject:?}"
    );
}

#[test]
fn a_second_code_is_refused_as_a_map_or_chart_is() {
    let nodes = [
        Node::col(2, Size::grow(1)),
        tile(registry::CODE, 0, Size::CONTENT),
        tile(registry::CODE, 0, Size::CONTENT),
    ];
    let tall = Scaled::fitting(240, 400).unwrap();
    let reject = Screen::parse(1, &bytes(&nodes), &tall, Controls::FREE).unwrap_err();
    assert_eq!(reject.why, Why::Heavy { node: 2 }, "{reject}");
}

/// A static page's words and numbers, for the page tiles.
struct Page<'a>(PageData<'a>);

impl Data for Page<'_> {
    fn metric(&self, _: Metric) -> Option<i32> {
        None
    }
    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }
    fn list(&self, _: ListKind) -> u16 {
        0
    }
    fn page(&self) -> PageData<'_> {
        self.0
    }
}

/// Every page tile, for every arg and look, drawn at exactly that look on
/// every panel, plain and focused, with the widest words and numbers its
/// page can hand it: the longest name a workout keeps, numbers at the ends
/// they are held to (with the words a narrow card sheds shed), every fixed
/// line. It draws all of it, inside its box.
#[test]
fn a_page_tile_at_its_smallest_box_shows_its_widest_content_whole() {
    use crate::screens::home::{Home, NONE_SAVED, last_ride, saved, this_week};
    use crate::screens::phone::{self, NAME, NOTE, PROBLEM, SECRET, STEP, TITLE_LINE};
    use crate::screens::qr::Qr;
    use crate::screens::ready::{Check, Level, Readiness};
    use crate::screens::sensors::Glance;
    use crate::screens::setup::{STEPS, narrow, widest_note};
    use crate::sensors::ant::device_type as ant;
    use crate::workout::model::{Name, WIDEST_NAME};
    let names = [
        "WWWWWWWWWWWWWWWWWWWW",
        "MMMMMMMMMM MMMMMMMMMM",
        WIDEST_NAME,
        "",
    ]
    .map(Name::new);
    let text = |line: &str| -> Text<38> {
        let mut text = Text::new();
        text.push_str(line).unwrap();
        text
    };
    let sensors = [
        ant::SPEED_CADENCE,
        ant::BIKE_LIGHTS,
        ant::SHIFTING,
        ant::SPEED_CADENCE,
    ]
    .map(|kind| Glance::ble(kind, true, None));
    let statuses = [
        names[0].as_str(),
        names[1].as_str(),
        "SENSORS UNAVAILABLE",
        "LIGHT CRITICAL BATTERY",
        "10 BATTERIES TO CHANGE",
    ];
    let lasts = [this_week(u64::MAX), last_ride(u64::MAX), saved(usize::MAX)];
    let mut homes = Vec::new();
    for (index, status) in statuses.iter().enumerate() {
        let last = lasts[index % lasts.len()].clone();
        let mut home = Home::new(index % 2 == 0, &sensors[..index % 5], last);
        home.set_status(status.as_bytes(), index % 2 == 1);
        homes.push(home);
    }
    let mut none = Home::new(false, &[], Text::new());
    none.last.push_str(NONE_SAVED).unwrap();
    homes.push(none);
    let checks = [
        ("STORAGE NOT FORMATTED", "OFF AND ON TO FORMAT"),
        ("UNDER 1 MIN OF ROOM", "LAST RIDE RECOVERED"),
        ("SENSORS UNAVAILABLE", "TURN VANA OFF AND ON"),
        ("SENSORS WAKING UP", "HR  PWR  SPD/C  RDR"),
        ("CLEAR RIDES FIRST", "CHARGE AFTER THE RIDE"),
    ];
    let readies: Vec<Readiness> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let check = |at: usize| {
                let (title, hint) = checks[(index + at) % checks.len()];
                Check::new(Level::Wait, title, hint)
            };
            let mut ready = Readiness {
                riding: Text::new(),
                checks: [check(0), check(1), check(2), check(3)],
            };
            ready.riding.push_str(name.as_str()).unwrap();
            ready
        })
        .collect();
    // A card's smallest box holds its note with words shed, as it shows there.
    let notes: Vec<[Text<38>; 1]> = STEPS
        .iter()
        .map(|&step| {
            let note = widest_note(step);
            [text(
                core::str::from_utf8(narrow(step, note.as_bytes())).unwrap(),
            )]
        })
        .collect();
    let phones: Vec<[Text<38>; 6]> = [
        [
            "PHONE DISCONNECTED",
            "ENDED AFTER 10 MIN IDLE",
            "",
            "",
            "TAP TO TRY AGAIN",
            "1",
        ],
        [
            "STEP 1 OF 2",
            "JOIN VANA WI-FI",
            "WAITING FOR WI-FI",
            "ABCD EFGH JKMN",
            "ENDS AFTER 10 MIN IDLE",
            "",
        ],
        [
            "STEP 2 OF 2",
            "OPEN IN A BROWSER",
            "255.255.255.255",
            "",
            "ENDS AFTER 10 MIN IDLE",
            "",
        ],
        // Passwords and networks at their widest and lowest glyphs.
        [
            "STEP 1 OF 2",
            "JOIN VANA WI-FI",
            "VANA-DDDD",
            "WWWW WWWW WWWW",
            "SCAN WITH THE CAMERA",
            "",
        ],
        [
            "STEP 1 OF 2",
            "JOIN VANA WI-FI",
            "VANA-0000",
            "WMWM WMNW MWWW",
            "9 JOINED",
            "",
        ],
        [
            "STEP 1 OF 2",
            "JOIN VANA WI-FI",
            "VANA-BBBB",
            "QQQQ QQQQ QQQQ",
            "",
            "",
        ],
    ]
    .iter()
    .map(|lines| {
        let mut out: [Text<38>; 6] = Default::default();
        for (at, line) in [STEP, TITLE_LINE, NAME, SECRET, NOTE, PROBLEM]
            .into_iter()
            .zip(lines)
        {
            out[at] = text(line);
        }
        out
    })
    .collect();
    let code = Qr::encode("WIFI:T:WPA;S:VANA-ABCD;P:ABCDEFGHJKMN;;").unwrap();
    let (mut checked, mut faults) = (0, Vec::new());
    for at in panels() {
        for component in [
            registry::MARK,
            registry::SAYS,
            registry::CODE,
            registry::CHOICE,
            registry::CHECK,
            registry::RIDING,
            registry::HOME_CARD,
        ] {
            let tile = registry::tile(component);
            for arg in (0..=255).filter(|&arg| tile.accepts(arg)) {
                for (form, look) in tile.forms(arg, &at).iter().enumerate() {
                    let draw = |width: u16, focused: bool, world: usize| {
                        let rect = Rect16 {
                            left: 4,
                            top: 4,
                            right: 4 + width,
                            bottom: 4 + look.height,
                        };
                        let placed = Placed {
                            rect,
                            node: 0,
                            form: form as u8,
                        };
                        let qr =
                            (component == registry::CODE).then(|| phone::place(rect.rect(), code));
                        let pages = homes
                            .iter()
                            .map(PageData::Home)
                            .chain(readies.iter().map(PageData::Ready))
                            .chain(STEPS.iter().zip(&notes).flat_map(|(&step, notes)| {
                                [
                                    PageData::Setup { step, notes },
                                    PageData::Setup { step, notes: &[] },
                                ]
                            }))
                            .chain(phones.iter().flat_map(|lines| {
                                [false, true].map(|light| PageData::Phone {
                                    lines,
                                    qr: qr.as_ref(),
                                    light,
                                })
                            }))
                            .chain([0, 500, 1000].map(PageData::Brand));
                        let page = pages.clone().nth(world)?;
                        let mut canvas = Canvas::default();
                        if focused {
                            tile.focused(arg, &placed, &at, &Page(page), &mut canvas);
                        } else {
                            tile.prepare(arg, &placed, &at, &Page(page), &mut canvas);
                        }
                        Some((rect.rect(), canvas))
                    };
                    for world in 0.. {
                        let Some((rect, _)) = draw(look.width, false, world) else {
                            break;
                        };
                        for focused in [false, true] {
                            let what = format!(
                                "{}({arg}) look {form} on {:?}, world {world}{}",
                                tile.name(),
                                at.panel,
                                if focused { " focused" } else { "" }
                            );
                            let (_, exact) = draw(look.width, focused, world).unwrap();
                            let (_, roomy) = draw(look.width * 2 + 8, focused, world).unwrap();
                            drawn_whole_inside(&what, rect, &exact, &roomy, &mut faults);
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    faults.dedup();
    assert!(
        faults.is_empty(),
        "{} faults:\n{}",
        faults.len(),
        faults[..faults.len().min(40)].join("\n")
    );
    assert!(checked > 1000, "{checked} draws");
}
