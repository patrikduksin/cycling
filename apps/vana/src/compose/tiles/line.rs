//! One line of words on a static screen: a question, what its answer does,
//! a title, a note. Each line names its faces, best first, its colour and
//! its side; a line that states a number reads it through `Data::count`.
//! Its looks are sized by the widest text it can show.
use crate::compose::data::{Count, Data};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{AMBER, Align, GREEN, SECONDARY, WHITE};
use crate::screens::format::Goes;
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole::{self, Body, Caption, Title};
use firmware_shell::rendering::font::SPILL;

pub struct Line;

type Words = Text<24>;

macro_rules! says {
    ($($say:ident),* $(,)?) => {
        /// Every line a static screen says, by the byte a tree stores.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum Say { $($say),* }

        impl Say {
            const ALL: &'static [Self] = &[$(Self::$say),*];
            const NAMES: &'static [&'static str] = &[$(stringify!($say)),*];
        }
    };
}

says! {
    LoseCount,
    LoseRides,
    LoseLine,
    FormatTitle,
    FormatMade,
    FormatGoes,
    FormatWait,
    TurnOff,
    RidesSaved,
    SyncFirst,
    TurningOffIn,
    PressAny,
    StayOn,
    ChargeState,
}

/// How a line looks: its faces, best first, its colour and its side.
struct Look {
    faces: &'static [FaceRole],
    color: u16,
    align: Align,
}

const fn look(faces: &'static [FaceRole], color: u16) -> Look {
    Look {
        faces,
        color,
        align: Align::Center,
    }
}

impl Say {
    const fn look(self) -> Look {
        use Say::*;
        match self {
            LoseCount | LoseRides | FormatTitle | TurnOff => look(&[Title, Body], WHITE),
            RidesSaved | PressAny | StayOn => look(&[Body, Caption], SECONDARY),
            SyncFirst => look(&[Body, Caption], AMBER),
            TurningOffIn => look(&[Body], AMBER),
            ChargeState => look(&[Body], GREEN),
            FormatMade | FormatWait => look(&[Body, Caption], WHITE),
            FormatGoes | LoseLine => look(&[Caption], SECONDARY),
        }
    }

    /// The words for `data`, or none where the line shows nothing now.
    fn write(self, data: &dyn Data, out: &mut Words) -> Option<()> {
        use Say::*;
        let count = |count| data.count(count);
        let plural = || count(Count::Unsaved) != Some(1);
        let text: &str = match self {
            LoseCount => {
                let unsaved = count(Count::Unsaved)?.clamp(0, i32::from(u8::MAX));
                let _ = write!(out, "LOSE {unsaved} UNSAVED");
                return Some(());
            }
            LoseRides if plural() => "RIDES?",
            LoseRides => "RIDE?",
            LoseLine if plural() => "THEY CANNOT COME BACK",
            LoseLine => "IT CANNOT COME BACK",
            FormatGoes => Goes::ALL
                .get(usize::try_from(count(Count::Goes)?).ok()?)?
                .text(),
            SyncFirst if plural() => "SYNC RIDES FIRST",
            SyncFirst => "SYNC RIDE FIRST",
            ChargeState => match (count(Count::Charging) == Some(1), count(Count::Charge)) {
                (_, Some(100..)) => "CHARGED",
                (true, _) => "CHARGING",
                (false, _) => "ON USB POWER",
            },
            _ => self.fixed(),
        };
        let _ = out.push_str(text);
        Some(())
    }

    /// The line's colour for `data`.
    fn tone(self, data: &dyn Data, color: u16) -> u16 {
        let idle = data.count(Count::Charging) != Some(1);
        match self {
            Self::ChargeState if idle && data.count(Count::Charge) != Some(100) => SECONDARY,
            _ => color,
        }
    }

    /// The words of a line that never changes.
    const fn fixed(self) -> &'static str {
        use Say::*;
        match self {
            FormatTitle => "FORMAT STORAGE?",
            FormatMade => "MADE BY ANOTHER VERSION",
            FormatWait => "CHECKING STORAGE",
            TurnOff => "TURN OFF",
            RidesSaved => "RIDES ARE SAVED",
            TurningOffIn => "TURNING OFF IN",
            PressAny => "PRESS ANY BUTTON",
            StayOn => "TO STAY ON",
            _ => "",
        }
    }

    /// Every text the line can show at its widest, for its looks.
    fn widest(self, each: &mut dyn FnMut(&[u8])) {
        use Say::*;
        match self {
            LoseCount => each(b"LOSE 255 UNSAVED"),
            LoseRides => each(b"RIDES?"),
            LoseLine => each(b"THEY CANNOT COME BACK"),
            FormatGoes => Goes::ALL
                .iter()
                .for_each(|goes| each(goes.text().as_bytes())),
            SyncFirst => each(b"SYNC RIDES FIRST"),
            ChargeState => ["CHARGED", "CHARGING", "ON USB POWER"]
                .iter()
                .for_each(|state| each(state.as_bytes())),
            _ => each(self.fixed().as_bytes()),
        }
    }
}

impl Arg for Say {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

impl Tile for Line {
    type Arg = Say;
    const NAME: &'static str = "line";

    fn forms(say: Say, at: &Scaled) -> Forms {
        say.look()
            .faces
            .iter()
            .map(|&role| {
                let face = at.face(role);
                let mut widest = Form::text(face, b"");
                say.widest(&mut |text| {
                    let form = Form::text(face, text);
                    if form.width > widest.width {
                        widest = form;
                    }
                });
                widest
            })
            .collect()
    }

    fn prepare(say: Say, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let mut words = Words::new();
        if say.write(data, &mut words).is_none() {
            return;
        }
        let look = say.look();
        let face = at.face(look.faces[usize::from(placed.form)]);
        let rect = placed.rect;
        let (left, width) = (usize::from(rect.left), usize::from(rect.width()));
        let words = words.as_bytes();
        let (x, align) = match look.align {
            Align::Center => (left + centred(face.fit_width(words), width), Align::Left),
            Align::Right => (left + width - SPILL, Align::Right),
            Align::Left => (left, Align::Left),
        };
        let base = usize::from(rect.bottom);
        out.fit(
            &[face],
            words,
            width,
            x,
            base,
            align,
            say.tone(data, look.color),
        );
    }
}

/// Columns before a line `drawn` wide centred in `width`.
const fn centred(drawn: usize, width: usize) -> usize {
    width.saturating_sub(drawn) / 2
}
