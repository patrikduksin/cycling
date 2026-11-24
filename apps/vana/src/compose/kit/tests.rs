use super::field::Field;
use super::{field, grid};
use crate::compose::input::Controls;
use crate::compose::node::{Flags, Node, Size, What};
use crate::compose::parse::Screen;
use crate::compose::registry::{FIELD, RULE, tile};
use crate::compose::solve::Solved;
use crate::screens::chrome::Rect;
use crate::screens::scaled::BASE;
use std::vec::Vec;

/// A 2 × 3 grid's boxes on the base panel, each with its tile kind.
fn solved() -> Vec<(u8, Rect)> {
    let rows = [
        [field(Field::Pwr), field(Field::Heart)],
        [field(Field::Spd), field(Field::Cad)],
        [field(Field::Dist), field(Field::Np)],
    ];
    let nodes: Vec<Node> = grid(&rows, Size::grow(1)).collect();
    let bytes: Vec<u8> = nodes.iter().flat_map(|node| node.bytes()).collect();
    let screen = Screen::parse(0, &bytes, &BASE, Controls::FREE).unwrap();
    let mut out = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut out);
    out.leaves()
        .iter()
        .filter_map(|placed| match nodes[usize::from(placed.node)].what()? {
            What::Leaf { component, .. } => Some((component, placed.rect.rect())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_grid_shares_its_room_evenly_and_its_rules_line_up() {
    let boxes = solved();
    let (width, height) = (BASE.panel.width, BASE.panel.height);
    let kind = |want| -> Vec<Rect> {
        boxes
            .iter()
            .filter(|(component, _)| *component == want)
            .map(|(_, rect)| *rect)
            .collect()
    };
    let (fields, rules) = (kind(FIELD), kind(RULE));
    assert_eq!(fields.len(), 6);
    assert_eq!(rules.len(), 3 + 2, "a rule in each row, one between rows");
    let across: Vec<_> = rules.iter().filter(|r| r.height() == 1).collect();
    let down: Vec<_> = rules.iter().filter(|r| r.width() == 1).collect();
    assert_eq!((across.len(), down.len()), (2, 3));
    assert!(across.iter().all(|r| r.left == 0 && r.right == width));
    assert!(down.windows(2).all(|pair| pair[0].left == pair[1].left));
    let spread = |sizes: Vec<usize>| sizes.iter().max().unwrap() - sizes.iter().min().unwrap();
    assert!(spread(fields.iter().map(Rect::height).collect()) <= 1);
    assert!(spread(fields.iter().map(Rect::width).collect()) <= 1);
    let bottom = boxes.iter().map(|(_, r)| r.bottom).max().unwrap();
    assert_eq!(bottom, height, "the grid fills the panel");
}

#[test]
fn a_field_names_itself_by_its_title_and_reads_its_metric() {
    let kind = tile(FIELD);
    let mut label = std::string::String::new();
    kind.label(Field::Heart as u8, &mut label).unwrap();
    assert_eq!(label, "HEART BPM");
    for &field in Field::ALL {
        assert!(kind.accepts(field as u8), "{field:?}");
        assert!(kind.reads(field as u8).is_some(), "{field:?}");
    }
}
