//! One centred line of a ride question or of the paused screen: a title, a
//! reading with its words, or a hint. Its baseline is the box's bottom.
use crate::compose::data::{Data, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{AMBER, Align, SECONDARY, WHITE};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct RideLine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Says {
    /// PAUSED, or AUTO PAUSED while the ride paused itself.
    Paused,
    /// How long the pause has lasted.
    PausedFor,
    RideTime,
    RideKm,
    /// RIDE ON TO RESUME, only while the ride paused itself.
    Resume,
    EndRide,
    /// What ending the ride does.
    Saved,
}

impl Says {
    const ALL: [Self; 7] = [
        Self::Paused,
        Self::PausedFor,
        Self::RideTime,
        Self::RideKm,
        Self::Resume,
        Self::EndRide,
        Self::Saved,
    ];

    pub const fn byte(self) -> u8 {
        self as u8
    }

    fn faces(self) -> &'static [FaceRole] {
        match self {
            Self::Paused | Self::EndRide => &[FaceRole::Title, FaceRole::Body],
            Self::RideTime => &[FaceRole::Large, FaceRole::Value],
            Self::PausedFor | Self::RideKm => &[FaceRole::Body],
            Self::Resume | Self::Saved => &[FaceRole::Caption],
        }
    }

    const fn color(self) -> u16 {
        match self {
            Self::Paused => AMBER,
            Self::RideTime | Self::RideKm | Self::EndRide => WHITE,
            Self::PausedFor | Self::Resume | Self::Saved => SECONDARY,
        }
    }

    pub const fn metric(self) -> Option<Metric> {
        match self {
            Self::PausedFor => Some(Metric::Paused),
            Self::RideTime => Some(Metric::Elapsed),
            Self::RideKm => Some(Metric::Distance),
            Self::Paused | Self::Resume | Self::EndRide | Self::Saved => None,
        }
    }

    fn write(self, reading: &str, auto_paused: bool, out: &mut Text<24>) {
        let _ = match self {
            Self::Paused if auto_paused => out.push_str("AUTO PAUSED"),
            Self::Paused => out.push_str("PAUSED"),
            Self::PausedFor => write!(out, "FOR {reading}"),
            Self::RideTime => out.push_str(reading),
            Self::RideKm => write!(out, "{reading} KM"),
            Self::Resume if auto_paused => out.push_str("RIDE ON TO RESUME"),
            Self::Resume => Ok(()),
            Self::EndRide => out.push_str("END RIDE?"),
            Self::Saved => out.push_str("YOUR RIDE WILL BE SAVED"),
        };
    }

    fn widest(self, face: &Face) -> usize {
        let readings = self
            .metric()
            .map_or_else(Default::default, Metric::extremes);
        let mut widest = 0;
        for reading in &readings {
            for auto_paused in [false, true] {
                let mut text = Text::<24>::new();
                self.write(reading.as_str(), auto_paused, &mut text);
                widest = widest.max(face.fit_width(text.as_bytes()));
            }
        }
        widest
    }
}

impl Arg for Says {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self.byte()
    }
}

impl Tile for RideLine {
    type Arg = Says;
    const NAME: &'static str = "ride line";

    fn reads(says: Says) -> Option<Reads> {
        says.metric().map(Reads::Metric)
    }

    fn forms(says: Says, at: &Scaled) -> Forms {
        let faces = says.faces();
        faces
            .iter()
            .map(|&role| {
                let face = at.face(role);
                // AUTO PAUSED may step down a face; PAUSED keeps the title face.
                let width = match says {
                    Says::Paused if role != faces[faces.len() - 1] => {
                        let fits = |text: &[u8], face: &Face| face.fit_width(text);
                        let smaller = at.face(faces[faces.len() - 1]);
                        fits(b"PAUSED", face).max(fits(b"AUTO PAUSED", smaller))
                    }
                    _ => says.widest(face),
                };
                Form {
                    width: width as u16,
                    height: face.fit_height() as u16,
                    shapes: 0,
                    runs: 1,
                }
            })
            .collect()
    }

    fn prepare(says: Says, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let mut reading = Text::<12>::new();
        if let Some(metric) = says.metric() {
            metric.write(data.metric(metric), &mut reading);
        }
        let mut text = Text::<24>::new();
        says.write(reading.as_str(), data.auto_paused(), &mut text);
        if text.as_bytes().is_empty() {
            return;
        }
        let rect = placed.rect;
        let (left, right) = (usize::from(rect.left), usize::from(rect.right));
        let faces: heapless::Vec<&'static Face, 2> = says.faces()[usize::from(placed.form)..]
            .iter()
            .map(|&role| at.face(role))
            .collect();
        out.fit(
            &faces,
            text.as_bytes(),
            right - left,
            (left + right - SPILL) / 2,
            usize::from(rect.bottom),
            Align::Center,
            says.color(),
        );
    }
}
