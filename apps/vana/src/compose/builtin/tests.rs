use super::*;
use crate::compose::input::{Action, Go};
use crate::screens::panels::ALL;
use std::vec::Vec;

#[test]
fn every_built_in_is_accepted_on_every_panel_with_its_buttons() {
    let mut accepted = 0;
    for id in ScreenId::ALL {
        let Some(builtin) = id.builtin() else {
            continue;
        };
        if id.deck() {
            let next = builtin.buttons.move_.map(|button| button.action);
            assert_eq!(next, Some(Action::Go(Go::Next)), "{id:?} is a deck page");
        }
        let bytes: Vec<u8> = (builtin.tree)(&Choices::default())
            .iter()
            .flat_map(|node| node.bytes())
            .collect();
        for layout in ALL {
            let controls = Controls::new(builtin.buttons, layout.corners);
            if let Err(reject) = Screen::parse(id.key(), &bytes, &layout.scaled, controls) {
                panic!("{id:?} on {:?}: {reject}", layout.scaled.panel);
            }
            accepted += 1;
        }
    }
    assert!(accepted >= ALL.len(), "the main page at least");
}

#[test]
fn every_screen_maps_to_its_own_id() {
    for (index, id) in ScreenId::ALL.into_iter().enumerate() {
        assert_eq!(usize::from(id.key()), index);
    }
}
