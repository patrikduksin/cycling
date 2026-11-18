//! FORMAT STORAGE?: what boot found made by another version, and what
//! formatting takes, centred down the panel as END RIDE? is; and the wait
//! while storage is checked.
use crate::screens::Layout;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, Rect, SECONDARY, WHITE};
use crate::screens::scaled::{Scaled, Track};
use firmware_shell::rendering::font::FaceRole;

/// The question on one panel: rows whose capitals end on their bottoms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub title: Rect,
    /// MADE BY ANOTHER VERSION, wider than the title.
    pub made: Rect,
    /// What goes, as wide.
    pub goes: Rect,
    /// CHECKING STORAGE, in the title's columns.
    pub wait: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, text, _] = screen.cols([Unit(24), Fr(1), Unit(24)], Unit(0), at);
    let [_, wide, _] = screen.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [_, title, made, goes, _, _] = screen.rows(
        [Fr(1), Unit(112), Unit(66), Unit(66), Unit(76), Fr(1)],
        Unit(0),
        at,
    );
    Regions {
        title: title.with_columns(text),
        made: made.with_columns(wide),
        goes: goes.with_columns(wide),
        wait: made.with_columns(text),
    }
}

pub const TITLE_TEXT: &str = "FORMAT STORAGE?";
pub const MADE_ELSEWHERE: &str = "MADE BY ANOTHER VERSION";
pub const WAIT: &str = "CHECKING STORAGE";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shows {
    Question(Goes),
    #[default]
    Wait,
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

pub fn prepare(layout: &Layout, shows: Shows) -> Canvas {
    let (at, regions) = (&layout.scaled, &layout.format);
    let [title, body, caption] =
        [FaceRole::Title, FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    let mut canvas = Canvas::default();
    let mut line = |faces: &[_], text: &str, row: Rect, color| {
        let (width, x) = (row.width(), row.center_x());
        canvas.fit(
            faces,
            text.as_bytes(),
            width,
            x,
            row.bottom,
            Align::Center,
            color,
        );
    };
    let goes = match shows {
        Shows::Question(goes) => goes,
        Shows::Wait => {
            line(&[body, caption], WAIT, regions.wait, WHITE);
            return canvas;
        }
    };
    line(&[title, body], TITLE_TEXT, regions.title, WHITE);
    line(&[body, caption], MADE_ELSEWHERE, regions.made, WHITE);
    line(&[caption], goes.text(), regions.goes, SECONDARY);
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
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
