//! A workout's played steps as Home's strip draws them: how long each is
//! and how hard, as a percent of FTP.
use super::model::{Length, Target, Workout};
use crate::ride::zones::TrainingProfile;

/// Played steps a shape keeps; a longer workout's last steps go.
pub const PIECES: usize = 24;

/// A workout's played steps as Home's strip draws them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shape {
    pieces: [Piece; PIECES],
    len: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Piece {
    pub secs: u16,
    /// Percent of FTP, 0 for a step with no power or heart target.
    pub pct: u8,
}

const DRAWN_METRES_A_SEC: u32 = 7;
const DRAWN_OPEN_SECS: u16 = 300;
const HEART_ZONE_FTP_PCT: [u8; 5] = [50, 65, 83, 98, 113];

impl Shape {
    pub fn of(workout: &Workout) -> Self {
        let mut shape = Self::default();
        for (slot, played) in shape.pieces.iter_mut().zip(workout.played()) {
            *slot = Piece::of(played.step.length, played.step.target);
            shape.len += 1;
        }
        shape
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.pieces[..usize::from(self.len)]
    }
}

impl Piece {
    fn of(length: Length, target: Target) -> Self {
        let secs = match length {
            Length::Time(secs) => secs,
            Length::Distance(metres) => {
                (metres / DRAWN_METRES_A_SEC).min(u32::from(u16::MAX)) as u16
            }
            Length::Open => DRAWN_OPEN_SECS,
        };
        // The outline is made without the rider's profile: watts and heart
        // rates are drawn against the default one.
        let profile = TrainingProfile::default();
        let of_watts = |watts: u16| u32::from(watts) * 100 / u32::from(profile.ftp_watts);
        let heart = |zone: u8| HEART_ZONE_FTP_PCT[usize::from(zone.clamp(1, 5) - 1)];
        let pct = match target {
            Target::Free | Target::Cadence { .. } => 0,
            Target::Ftp { low, high } => (u32::from(low) + u32::from(high)) / 2,
            Target::Watts { low, high } => of_watts(low / 2 + high / 2),
            Target::HeartZone(zone) => u32::from(heart(zone)),
            Target::Lthr { low, high } => {
                let percent = TrainingProfile {
                    lthr_bpm: 100,
                    ..profile
                };
                u32::from(heart(percent.heart_zone(u16::from(low / 2 + high / 2))))
            }
            Target::Ramp { start, .. } => of_watts(start),
            Target::FtpRamp { start, .. } => u32::from(start),
        };
        Self {
            secs,
            pct: pct.min(u32::from(u8::MAX)) as u8,
        }
    }
}
