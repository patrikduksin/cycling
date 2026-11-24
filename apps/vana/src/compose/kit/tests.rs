use super::field::Field;
use super::{field, grid};
use crate::compose::input::Controls;
use crate::compose::node::{Flags, Node, Size};
use crate::compose::parse::Screen;
use crate::compose::registry::{FIELD, RULE, tile};
use crate::compose::solve::Solved;
use crate::screens::scaled::BASE;
use std::vec::Vec;

fn bytes(nodes: &[Node]) -> Vec<u8> {
    nodes.iter().flat_map(|node| node.bytes()).collect()
}

/// A 2 × 3 grid's boxes: each field's, each rule's.
fn solved() -> (Vec<(u8, u8, crate::screens::chrome::Rect)>, (usize, usize)) {
    let rows = [
        [field(Field::Pwr), field(Field::Heart)],
        [field(Field::Spd), field(Field::Cad)],
        [field(Field::Dist), field(Field::Np)],
    ];
    let nodes: Vec<Node> = grid(&rows, Size::grow(1)).collect();
    let screen = Screen::parse(0, &bytes(&nodes), &BASE, Controls::FREE).unwrap();
    let mut out = Solved::EMPTY;
    screen.solve(Flags::NONE, &mut out);
    let boxes = out
        .leaves()
        .iter()
        .map(|placed| {
            let node = nodes[usize::from(placed.node)];
            let crate::compose::node::What::Leaf { component, arg } = node.what().unwrap() else {
                unreachable!("only leaves are placed");
            };
            (component, arg, placed.rect.rect())
        })
        .collect();
    (boxes, (BASE.panel.width, BASE.panel.height))
}

#[test]
fn a_grid_shares_its_room_evenly_and_its_rules_line_up() {
    let (boxes, (width, height)) = solved();
    let fields: Vec<_> = boxes.iter().filter(|(c, ..)| *c == FIELD).collect();
    let rules: Vec<_> = boxes.iter().filter(|(c, ..)| *c == RULE).collect();
    assert_eq!(fields.len(), 6);
    assert_eq!(rules.len(), 3 + 2, "a rule in each row, one between rows");
    let across: Vec<_> = rules.iter().filter(|(.., r)| r.height() == 1).collect();
    let down: Vec<_> = rules.iter().filter(|(.., r)| r.width() == 1).collect();
    assert_eq!((across.len(), down.len()), (2, 3));
    assert!(across.iter().all(|(.., r)| r.left == 0 && r.right == width));
    assert!(down.windows(2).all(|pair| pair[0].2.left == pair[1].2.left));
    let heights: Vec<usize> = fields.iter().map(|(.., r)| r.height()).collect();
    assert!(heights.iter().max().unwrap() - heights.iter().min().unwrap() <= 1, "{heights:?}");
    let widths: Vec<usize> = fields.iter().map(|(.., r)| r.width()).collect();
    assert!(widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1, "{widths:?}");
    let bottom = boxes.iter().map(|(.., r)| r.bottom).max().unwrap();
    assert_eq!(bottom, height, "the grid fills the panel");
}

#[test]
fn zz_measure() {
    use firmware_shell::rendering::font::FaceRole;
    let at = BASE;
    let tag = at.face(FaceRole::Tag);
    for t in ["HEART BPM", "PWR 3S W", "Z7", "SPD KM/H", "1888", "88.8", "18.8"] {
        std::println!(
            "{t}: tag {} d40 {} d26 {}",
            tag.fit_width(t.as_bytes()),
            at.face(FaceRole::Digits40).fit_width(t.as_bytes()),
            at.face(FaceRole::Digits26).fit_width(t.as_bytes())
        );
    }
    for s in [crate::screens::icons::Symbol::Heart, crate::screens::icons::Symbol::Power] {
        std::println!("{s:?} {:?}", at.art.symbol(s).ink(0, 0));
    }
    for &field in Field::ALL {
        let forms = tile(FIELD).forms(field as u8, &at);
        std::println!("{field:?} {:?}", forms[0]);
    }
    let d40 = at.face(FaceRole::Digits40);
    std::println!("tag h {} d40 h {} cap {}", tag.fit_height(), d40.fit_height(), d40.cap);
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
