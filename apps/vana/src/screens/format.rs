//! FORMAT STORAGE?: what boot found made by another version, and what
//! formatting takes; and the wait while storage is checked.

pub const TITLE_TEXT: &str = "FORMAT STORAGE?";
pub const MADE_ELSEWHERE: &str = "MADE BY ANOTHER VERSION";
pub const WAIT: &str = "CHECKING STORAGE";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shows {
    Question(Goes),
    #[default]
    Wait,
}

impl Shows {
    pub const fn asks(self) -> crate::compose::builtin::Asks {
        match self {
            Self::Question(_) => crate::compose::builtin::Asks::Own,
            Self::Wait => crate::compose::builtin::Asks::Wait,
        }
    }

    /// What formatting takes, as its place in `Goes::ALL`.
    pub fn goes(self) -> Option<u8> {
        let Self::Question(goes) = self else {
            return None;
        };
        Goes::ALL
            .iter()
            .position(|&it| it == goes)
            .map(|at| at as u8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goes {
    RidesAndSettings,
    WholeCard,
    CardRides,
    CardFiles,
    Traces,
    RideLog,
    Settings,
    RidesAndMore,
    SettingsAndMore,
    FilesAndMore,
    RideLogAndMore,
}

impl Goes {
    pub const ALL: [Self; 11] = [
        Self::RidesAndSettings,
        Self::WholeCard,
        Self::CardRides,
        Self::CardFiles,
        Self::Traces,
        Self::RideLog,
        Self::Settings,
        Self::RidesAndMore,
        Self::SettingsAndMore,
        Self::FilesAndMore,
        Self::RideLogAndMore,
    ];

    pub const fn text(self) -> &'static str {
        match self {
            Self::RidesAndSettings => "RIDES AND SETTINGS GO",
            Self::WholeCard => "WHOLE CARD GOES",
            Self::CardRides => "CARD RIDES GO",
            Self::CardFiles => "CARD FILES GO",
            Self::Traces => "TRACES GO",
            Self::RideLog => "DEVICE RIDE LOG GOES",
            Self::Settings => "SETTINGS GO",
            Self::RidesAndMore => "RIDES AND MORE GO",
            Self::SettingsAndMore => "SETTINGS AND MORE GO",
            Self::FilesAndMore => "FILES AND MORE GO",
            Self::RideLogAndMore => "RIDE LOG AND MORE GO",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::Rect;
    use crate::screens::workout::{Page, View};

    #[test]
    fn every_question_and_the_wait_lay_out_whole() {
        crate::screens::check::enable(true);
        for layout in crate::screens::panels::ALL {
            let at = &layout.scaled;
            let screen = at.screen();
            let clear = Rect {
                top: at.unit(50),
                bottom: screen.bottom - at.unit(50),
                ..screen
            };
            let shows = Goes::ALL.map(Shows::Question);
            for shows in shows.into_iter().chain([Shows::Wait]) {
                let mut view = View::new(Page::Format, 0, None);
                view.layout = layout;
                view.format = shows;
                view.compose_main();
                view.prepare().check_layout();
                let want: &[&str] = match shows {
                    Shows::Question(goes) => &[TITLE_TEXT, MADE_ELSEWHERE, goes.text()],
                    Shows::Wait => &[WAIT],
                };
                let what = std::format!("{shows:?} on {:?}", at.panel);
                view.canvas().assert_laid_out(clear, want, &what);
            }
        }
    }
}
