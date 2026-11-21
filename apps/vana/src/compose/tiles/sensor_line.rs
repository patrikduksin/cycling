//! One centred line of a sensor's pages: its name, its state, the power
//! meter's calibration and what to do about it, FORGET SENSOR?'s words and
//! the sensor it forgets, and REPLACE SENSOR's question.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, SECONDARY, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{CalibrationNote, kind, widest};
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole};

pub struct SensorLine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Says {
    Name,
    State,
    Calibration,
    Hint,
    Forget,
    Sensor,
    Forgotten,
    Replace,
}

impl Says {
    const ALL: [Self; 8] = [
        Self::Name,
        Self::State,
        Self::Calibration,
        Self::Hint,
        Self::Forget,
        Self::Sensor,
        Self::Forgotten,
        Self::Replace,
    ];

    /// Its faces, largest first: the line takes the largest its box fits.
    const fn faces(self) -> &'static [FaceRole] {
        match self {
            Self::Name => &[FaceRole::Body, FaceRole::Caption],
            Self::Forget | Self::Sensor => &[FaceRole::Title, FaceRole::Body],
            Self::State | Self::Calibration | Self::Hint | Self::Forgotten | Self::Replace => {
                &[FaceRole::Caption]
            }
        }
    }

    /// The columns the widest it may say covers in `face`, its smallest.
    fn widest(self, face: &Face) -> usize {
        match self {
            Self::Name => widest::name(face),
            Self::State => widest::state(face),
            Self::Calibration => widest::calibration(face),
            Self::Hint => {
                let hints = CalibrationNote::ALL
                    .into_iter()
                    .filter_map(CalibrationNote::hint);
                widest::of(face, hints.map(str::as_bytes))
            }
            Self::Forget => face.fit_width(b"FORGET"),
            Self::Sensor => face.fit_width(b"SENSOR?"),
            Self::Forgotten => widest::name(face) + face.width(b" ") + widest::identity(face),
            Self::Replace => face.fit_width(b"REPLACE SENSOR"),
        }
    }
}

impl Arg for Says {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }

    fn label(self) -> &'static str {
        match self {
            Self::Name => "NAME",
            Self::State => "STATE",
            Self::Calibration => "CALIBRATION",
            Self::Hint => "HINT",
            Self::Forget => "FORGET",
            Self::Sensor => "SENSOR",
            Self::Forgotten => "FORGOTTEN",
            Self::Replace => "REPLACE",
        }
    }
}

impl Tile for SensorLine {
    type Arg = Says;
    const NAME: &'static str = "sensor_line";

    fn forms(says: Says, at: &Scaled) -> Forms {
        let smallest = at.face(*says.faces().last().unwrap_or(&FaceRole::Caption));
        let width = says.widest(smallest) as u16;
        says.faces()
            .iter()
            .map(|&role| Form {
                width,
                ..Form::text(at.face(role), b"")
            })
            .collect()
    }

    fn prepare(says: Says, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let mut line = Text::<28>::new();
        let color = match says {
            Says::Name => {
                let Some(identity) = menu.pictured() else {
                    return;
                };
                let _ = line.push_str(name(identity.device_type));
                WHITE
            }
            Says::State => {
                let Some((text, color)) = menu.shown().map(|shown| shown.link()) else {
                    return;
                };
                let _ = line.push_str(core::str::from_utf8(text).unwrap_or(""));
                color
            }
            Says::Calibration => {
                let Some(note) = menu.calibration() else {
                    return;
                };
                let _ = line.push_str(note.text().as_str());
                note.color()
            }
            Says::Hint => {
                let Some(hint) = menu.calibration().and_then(|note| note.hint()) else {
                    return;
                };
                let _ = line.push_str(hint);
                SECONDARY
            }
            Says::Forget => {
                let _ = line.push_str("FORGET");
                WHITE
            }
            Says::Sensor => {
                let _ = line.push_str("SENSOR?");
                WHITE
            }
            Says::Forgotten => {
                let Some(shown) = menu.shown() else {
                    return;
                };
                let identity = shown.identity;
                let label = menu.identity_label(identity);
                let _ = write!(line, "{} {}", name(identity.device_type), label.as_str());
                SECONDARY
            }
            Says::Replace => {
                let _ = line.push_str("REPLACE SENSOR");
                SECONDARY
            }
        };
        let faces = &says.faces()[usize::from(placed.form)..];
        let first = at.face(faces[0]);
        let last = at.face(*faces.last().unwrap_or(&faces[0]));
        let rect = placed.rect.rect();
        out.fit(
            &[first, last],
            line.as_bytes(),
            rect.width(),
            rect.center_x(),
            rect.bottom,
            Align::Center,
            color,
        );
    }
}

fn name(device_type: u8) -> &'static str {
    core::str::from_utf8(kind(device_type)).unwrap_or("")
}
