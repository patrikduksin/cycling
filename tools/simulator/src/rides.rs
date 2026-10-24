//! Rides read back from simulated media with VANA's own journal decoder.
use crate::recording::{RideSummary, json};
use crate::route::{Point, from_e7, to_gpx};
use firmware_services::fit::Counter;
use std::fmt::Write as _;
use vana::ride::fit::{self, Plan, Refusal};
use vana::ride::log::{Entry, Kind, SLOT_SIZE, Sample, Slot, decode};

pub struct Ride {
    pub id: u32,
    pub entries: Vec<Entry>,
    slots: Vec<Slot>,
}

impl Ride {
    /// What the FIT export finds in this ride when the phone gives no start:
    /// its start time, or why the file is refused.
    pub fn fit_plan(&self) -> Result<Plan, Refusal> {
        fit::write_all(self.slots.iter(), self.id, None, &mut Counter::default())
    }

    pub fn samples(&self) -> impl Iterator<Item = &Sample> {
        self.entries
            .iter()
            .filter(|e| e.kind == Kind::Samples)
            .flat_map(|e| &e.samples[..usize::from(e.count)])
    }

    pub fn kinds(&self) -> Vec<Kind> {
        self.entries.iter().map(|e| e.kind).collect()
    }

    pub fn track(&self) -> Vec<Point> {
        self.samples()
            .filter_map(|s| s.location_e7)
            .map(|(lat, lon)| from_e7(lat, lon))
            .collect()
    }

    /// The recorded track as GPX, with UTC times when the device knew them.
    pub fn gpx(&self) -> String {
        let points: Vec<_> = self
            .samples()
            .filter_map(|s| {
                let (lat, lon) = s.location_e7?;
                let mut point = from_e7(lat, lon);
                point.ele = s.altitude_dm.map(|dm| f64::from(dm) / 10.0);
                Some((point, s.utc_ms.map(|ms| ms / 1000)))
            })
            .collect();
        to_gpx(&format!("VANA ride {}", self.id), &points)
    }

    /// The ride as the FIT activity file the device exports, or why it has none;
    /// `start_utc_ms` dates a ride recorded without a clock.
    pub fn fit(&self, start_utc_ms: Option<u64>) -> Result<Vec<u8>, Refusal> {
        struct Out(Vec<u8>);
        impl firmware_services::fit::Sink for Out {
            fn put(&mut self, bytes: &[u8]) {
                self.0.extend_from_slice(bytes);
            }
        }
        let mut out = Out(Vec::new());
        fit::write_all(self.slots.iter(), self.id, start_utc_ms, &mut out)?;
        Ok(out.0)
    }

    /// Distance along the recorded track, in metres.
    pub fn track_m(&self) -> f64 {
        self.track()
            .windows(2)
            .map(|w| crate::route::distance_m(w[0], w[1]))
            .sum()
    }
}

/// Every committed slot in journal order, grouped into rides. Torn or empty
/// slots are skipped, as the recorder's own scan does.
pub fn read(media: &[u8]) -> Vec<Ride> {
    let mut rides: Vec<Ride> = Vec::new();
    for bytes in media.as_chunks::<SLOT_SIZE>().0 {
        let Some(entry) = decode(&Slot(*bytes)) else {
            continue;
        };
        match rides.last_mut() {
            Some(ride) if ride.id == entry.ride_id => {
                ride.entries.push(entry);
                ride.slots.push(Slot(*bytes));
            }
            _ => rides.push(Ride {
                id: entry.ride_id,
                entries: vec![entry],
                slots: vec![Slot(*bytes)],
            }),
        }
    }
    rides
}

/// Files and figures a scenario report shows for one ride.
pub fn summary(ride: &Ride) -> RideSummary {
    let mut samples = String::new();
    for sample in ride.samples() {
        let field = |value: Option<String>| value.unwrap_or_else(|| "null".into());
        let _ = write!(
            samples,
            "{}\n    {{\"active_ms\": {}, \"lat\": {}, \"lon\": {}, \"speed_kmh\": {}, \"power_w\": {}, \"heart_bpm\": {}, \"cadence_rpm\": {}, \"gradient_pct\": {}, \"battery_pct\": {}, \"altitude_m\": {}}}",
            if samples.is_empty() { "" } else { "," },
            sample.active_ms,
            field(
                sample
                    .location_e7
                    .map(|(lat, _)| format!("{:.7}", f64::from(lat) / 1e7))
            ),
            field(
                sample
                    .location_e7
                    .map(|(_, lon)| format!("{:.7}", f64::from(lon) / 1e7))
            ),
            field(
                sample
                    .speed_mm_s
                    .map(|v| format!("{:.1}", f64::from(v) * 0.0036))
            ),
            field(sample.power_watts.map(|v| v.to_string())),
            field(sample.heart_bpm.map(|v| v.to_string())),
            field(
                sample
                    .cadence_tenths
                    .map(|v| format!("{:.1}", f64::from(v) / 10.0))
            ),
            field(
                sample
                    .gradient_tenths
                    .map(|v| format!("{:.1}", f64::from(v) / 10.0))
            ),
            field(sample.battery_percent.map(|v| v.to_string())),
            field(
                sample
                    .altitude_dm
                    .map(|v| format!("{:.1}", f64::from(v) / 10.0))
            ),
        );
    }
    let kinds: Vec<String> = ride.kinds().iter().map(|k| format!("{k:?}")).collect();
    RideSummary {
        id: ride.id,
        samples: ride.samples().count(),
        track_m: ride.track_m(),
        gpx: ride.gpx(),
        json: format!(
            "{{\n  \"ride\": {},\n  \"entries\": [{}],\n  \"samples\": [{}\n  ]\n}}\n",
            ride.id,
            kinds.iter().map(|k| json(k)).collect::<Vec<_>>().join(", "),
            samples
        ),
        kinds,
    }
}
