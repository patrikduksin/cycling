use super::*;
use crate::compose::input::{Action, Go};
use crate::screens::panels::ALL;
use std::vec::Vec;

fn every_arrangement() -> Vec<Keys> {
    use device_api::input::{Fixture, Fixtures, Role, Side};
    let spots = [
        (Side::Left, 250),
        (Side::Left, 750),
        (Side::Right, 250),
        (Side::Right, 750),
        (Side::Bottom, 217),
        (Side::Bottom, 783),
    ];
    let button = |id, role, (edge, at)| Fixture {
        id,
        role: Some(role),
        edge,
        at,
    };
    let mut arrangements = Vec::new();
    for act in spots {
        for back in spots.into_iter().filter(|&back| back != act) {
            for move_ in spots.into_iter().filter(|&it| it != act && it != back) {
                arrangements.push(Keys::of(&Fixtures::new(&[
                    button(0, Role::Act, act),
                    button(1, Role::Back, back),
                    button(2, Role::Move, move_),
                ])));
            }
        }
    }
    arrangements
}

#[test]
fn every_built_in_is_accepted_on_every_panel_for_every_button_arrangement() {
    let arrangements = every_arrangement();
    assert_eq!(arrangements.len(), 120);
    let mut refused = Vec::new();
    let mut accepted = 0;
    for id in ScreenId::ALL {
        // #517 deletes these; their full-width rows miss a few pixels
        // beside a side key's tab.
        let c606_only = matches!(id, ScreenId::Ready | ScreenId::Sheets);
        let builtin = id.builtin();
        if id.deck() {
            let next = builtin.buttons.move_.map(|button| button.action);
            assert_eq!(next, Some(Action::Go(Go::Next)), "{id:?} is a deck page");
        }
        let bytes: Vec<u8> = (builtin.tree)(&Choices::default())
            .iter()
            .flat_map(|node| node.bytes())
            .collect();
        for device in ALL {
            let c606 = crate::screens::C606.keys;
            for &keys in arrangements
                .iter()
                .filter(|&&keys| !c606_only || keys == c606)
            {
                let controls = Controls::new(builtin.buttons, keys);
                match Screen::parse(id.key(), &bytes, &device.scaled, controls) {
                    Ok(_) => accepted += 1,
                    Err(reject) => refused.push(std::format!(
                        "{id:?} on {:?}, {keys:?}: {reject}",
                        device.scaled.panel
                    )),
                }
            }
        }
    }
    assert!(refused.is_empty(), "refused:\n{}", refused.join("\n"));
    assert!(
        accepted >= ALL.len() * arrangements.len(),
        "the main page at least"
    );
}

#[test]
fn every_screen_maps_to_its_own_id() {
    for (index, id) in ScreenId::ALL.into_iter().enumerate() {
        assert_eq!(usize::from(id.key()), index);
    }
}
