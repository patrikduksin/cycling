//! LOSE n UNSAVED RIDES?: a host CLEAR would discard rides the phone has
//! not confirmed. ACT (trash) discards them with the journal; BACK keeps
//! them.
use super::question::{answer, block, gap, panel, say};
use super::{Builtin, Choices, Nodes};
use crate::compose::input::Buttons;
use crate::compose::tiles::line::Say;
use crate::screens::cue::Icon;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| TREE.into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: answer(true, Icon::Trash),
        back: answer(false, Icon::Back),
        move_: None,
        held: None,
    },
});

const TREE: [crate::compose::node::Node; 7] = [
    panel(1),
    block(5),
    say(Say::LoseCount),
    gap(10),
    say(Say::LoseRides),
    gap(16),
    say(Say::LoseLine),
];

#[cfg(test)]
mod tests {
    use crate::screens::chrome::Rect;
    use crate::screens::workout::{Page, View};

    #[test]
    fn the_question_counts_the_rides_it_would_lose_on_every_panel() {
        for layout in crate::screens::panels::ALL {
            for (unsaved, question, line) in [
                (1, ["LOSE 1 UNSAVED", "RIDE?"], "IT CANNOT COME BACK"),
                (2, ["LOSE 2 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
                (16, ["LOSE 16 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
                (255, ["LOSE 255 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
            ] {
                let mut view = View::new(Page::LoseRides, 0, None);
                view.layout = layout;
                view.unsaved = unsaved;
                view.compose_main();
                view.prepare().check_layout();
                let at = &layout.scaled;
                let screen = at.screen();
                let clear = Rect {
                    top: at.unit(50),
                    bottom: screen.bottom - at.unit(50),
                    ..screen
                };
                let want = [question[0], question[1], line];
                let what = std::format!("{} on {:?}", question[0], at.panel);
                view.canvas().assert_laid_out(clear, &want, &what);
            }
        }
    }
}
