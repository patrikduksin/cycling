use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, BODY, CAPTION, SECONDARY, TITLE, WHITE, WIDTH};

const CENTRE: usize = WIDTH / 2;
const TEXT_WIDTH: usize = WIDTH - 2 * 24;
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

pub fn prepare(shows: Shows) -> Canvas {
    let mut canvas = Canvas::default();
    let mut line = |faces: &[_], text: &str, width, y, color| {
        canvas.fit(
            faces,
            text.as_bytes(),
            width,
            CENTRE,
            y,
            Align::Center,
            color,
        );
    };
    let goes = match shows {
        Shows::Question(goes) => goes,
        Shows::Wait => {
            line(&[&BODY, &CAPTION], WAIT, TEXT_WIDTH, 178, WHITE);
            return canvas;
        }
    };
    line(&[&TITLE, &BODY], TITLE_TEXT, TEXT_WIDTH, 112, WHITE);
    line(&[&BODY, &CAPTION], MADE_ELSEWHERE, WIDTH - 20, 178, WHITE);
    line(&[&CAPTION], goes.text(), WIDTH - 20, 244, SECONDARY);
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{HEIGHT, Rect};

    #[test]
    fn every_question_and_the_wait_lay_out_whole() {
        let screen = Rect {
            left: 0,
            top: 50,
            right: WIDTH,
            bottom: HEIGHT - 50,
        };
        for goes in Goes::ALL {
            prepare(Shows::Question(goes)).assert_laid_out(
                screen,
                &[TITLE_TEXT, MADE_ELSEWHERE, goes.text()],
                goes.text(),
            );
        }
        prepare(Shows::Wait).assert_laid_out(screen, &[WAIT], "formatting");
    }
}
