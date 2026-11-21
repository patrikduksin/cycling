//! NEW FTP: the FTP a test found over whatever page it shows on, the one
//! in force under it, and SAVE?. ACT (check) saves it, BACK (back) keeps
//! the old one.
use super::question::{answer, block, gap, panel, say};
use super::{Builtin, Choices, Nodes};
use crate::compose::input::Buttons;
use crate::compose::node::{Node, Size};
use crate::compose::registry::FTP;
use crate::compose::tiles::line::Say;
use crate::screens::cue::Icon;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_: &Choices| TREE.into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: answer(true, Icon::Check),
        back: answer(false, Icon::Back),
        move_: None,
        held: None,
    },
};

const TREE: [Node; 9] = [
    panel(1),
    block(7),
    say(Say::NewFtp),
    gap(18),
    Node::leaf(FTP, 0, Size::CONTENT),
    gap(18),
    say(Say::FtpNow),
    gap(26),
    say(Say::FtpSave),
];

#[cfg(test)]
mod tests {
    use crate::screens::workout::{Page, View};

    #[test]
    fn the_sheet_lays_out_whole_with_its_answers_on_every_panel() {
        crate::screens::check::enable(true);
        for device in crate::screens::panels::ALL {
            let screen = device.scaled.screen();
            for (new, old) in [(247, 215), (600, 50), (50, 600), (999, 1000), (9999, 9999)] {
                let mut view = View::new(Page::Workouts, 0, None);
                view.device = device;
                view.ftp_offer = Some((new, old));
                view.compose_main();
                view.prepare().check_layout();
                let what = std::format!("sheet on {:?}", device.scaled.panel);
                let now = std::format!("NOW {old} W");
                let want = ["NEW FTP", "W", now.as_str(), "SAVE?"];
                view.canvas().assert_laid_out(screen, &want, &what);
                assert!(view.canvas().shows(&std::format!("{new}")), "{what}");
                let cues = view.prepare().cues().len();
                assert_eq!(cues, 2, "{what}: BACK keeps, ACT saves");
            }
        }
    }
}
