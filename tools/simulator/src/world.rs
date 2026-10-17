//! The rider and surroundings the simulated device observes. Values are what a
//! perfect sensor would measure; each simulated sensor encodes them its own way.
use crate::route::{Point, Route};

/// A vehicle the radar reports, in the rider's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vehicle {
    pub range_m: f64,
    pub closing_kmh: f64,
    pub fast: bool,
    pub side: Side,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Behind,
    Right,
    Left,
}

#[derive(Default)]
pub struct World {
    /// Simulated wall time, which keeps running across device restarts.
    pub time_ms: u64,
    pub speed_kmh: f64,
    pub heart_bpm: f64,
    pub power_w: f64,
    pub cadence_rpm: f64,
    /// Integrated distance, driven by `speed_kmh`.
    pub distance_m: f64,
    pub vehicles: Vec<Vehicle>,
    /// Barometric pressure follows elevation when set; a route with elevation sets it.
    pub elevation_m: Option<f64>,
    /// The barometer stops reporting while set.
    pub barometer_lost: bool,
    /// Where the rider is, by distance along this route.
    pub route: Option<Route>,
}

impl World {
    /// Advance motion by `ms`. Vehicles approach at their closing speed and pass.
    pub fn advance(&mut self, ms: u64) {
        let seconds = ms as f64 / 1000.0;
        self.time_ms += ms;
        self.distance_m += self.speed_kmh / 3.6 * seconds;
        if let Some(elevation) = self.position().and_then(|p| p.ele) {
            self.elevation_m = Some(elevation);
        }
        for vehicle in &mut self.vehicles {
            vehicle.range_m -= vehicle.closing_kmh / 3.6 * seconds;
        }
        self.vehicles.retain(|v| v.range_m > 0.0);
    }

    pub fn position(&self) -> Option<Point> {
        self.route.as_ref().map(|route| route.at(self.distance_m))
    }

    pub fn course(&self) -> f64 {
        self.route
            .as_ref()
            .map_or(0.0, |route| route.course(self.distance_m))
    }

    /// Standard-atmosphere pressure at the current elevation, in centipascals.
    pub fn pressure_centi_pa(&self) -> Option<u32> {
        if self.barometer_lost {
            return None;
        }
        let elevation = self.elevation_m?;
        let pascal = 101_325.0 * (1.0 - 2.255_77e-5 * elevation).powf(5.255_88);
        Some((pascal * 100.0) as u32)
    }
}
