use super::*;
use crate::compose::input::{Action, Go};
use crate::screens::panels::ALL;
use std::vec::Vec;

/// Every legal arrangement of the three roles' buttons: each role beside a
/// corner of its own.
fn every_arrangement() -> Vec<Corners> {
    use crate::screens::cue::Corner::{self, BottomLeft, BottomRight, TopLeft, TopRight};
    use device_api::input::{Fixture, Fixtures, Role, Side};
    let all = [TopLeft, TopRight, BottomLeft, BottomRight];
    let button = |id, role, corner: Corner| Fixture {
        id,
        role: Some(role),
        edge: if corner.left() {
            Side::Left
        } else {
            Side::Right
        },
        at: if corner.top() { 250 } else { 750 },
    };
    let mut arrangements = Vec::new();
    for act in all {
        for back in all.into_iter().filter(|&back| back != act) {
            for move_ in all.into_iter().filter(|&it| it != act && it != back) {
                arrangements.push(Corners::of(&Fixtures::new(&[
                    button(0, Role::Act, act),
                    button(1, Role::Back, back),
                    button(2, Role::Move, move_),
                ])));
            }
        }
    }
    arrangements
}

/// Accept keeps every tile clear of the corner cues, so a built-in that
/// passes here draws clear of its cues wherever a device puts its buttons.
#[test]
fn every_built_in_is_accepted_on_every_panel_for_every_button_arrangement() {
    let arrangements = every_arrangement();
    assert_eq!(arrangements.len(), 24);
    let mut refused = Vec::new();
    let mut accepted = 0;
    for id in ScreenId::ALL {
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
            for &corners in &arrangements {
                let controls = Controls::new(builtin.buttons, corners);
                match Screen::parse(id.key(), &bytes, &device.scaled, controls) {
                    Ok(_) => accepted += 1,
                    Err(reject) => refused.push(std::format!(
                        "{id:?} on {:?}, {corners:?}: {reject}",
                        device.scaled.panel
                    )),
                }
            }
        }
    }
    assert!(refused.is_empty(), "refused:\n{}", refused.join("\n"));
    assert!(accepted >= ALL.len() * 24, "the main page at least");
}

#[test]
fn every_screen_maps_to_its_own_id() {
    for (index, id) in ScreenId::ALL.into_iter().enumerate() {
        assert_eq!(usize::from(id.key()), index);
    }
}
