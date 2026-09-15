//! Physical sensor observations. Motion scaling and axes remain unverified.
use crate::observation::{Error, Observation};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pressure {
    /// Companion-compensated pressure in hundredths of a pascal.
    pub pressure_centi_pa: u32,
    /// Companion-compensated sensor temperature in hundredths of a degree Celsius.
    pub temperature_centi_c: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Signed wire-order values. Scaling and orientation are unverified.
    pub axes: [i16; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Opaque report fields, not a chip ID or an established firmware version.
    pub fields: [u8; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub pressure: Observation<Pressure>,
    /// Index zero is subtype one; index one is subtype two. No gyroscope claim.
    pub motion: [Observation<Motion>; 2],
    pub identity: Observation<Identity>,
    pub reports: u32,
    pub invalid_reports: u32,
    /// Number of observed transport discontinuities, not the number of lost pages.
    pub losses: u32,
}

/// Optional physical sensor observations; motion remains explicitly unscaled
/// until fitted hardware and axes are established.
pub trait Sensors {
    fn startup_status(&self) -> &'static str {
        "unsupported"
    }
    fn startup_reason(&self) -> Option<u8> {
        None
    }
    fn snapshot(&self, now: u64) -> crate::sensors::Snapshot;
    fn identity_status(&self) -> &'static str;
    fn query_identity(&mut self, now: u64) -> Result<(), Error>;
}
