//! Rides read back from simulated media with VANA's own journal decoder.
use crate::route::{Point, from_e7, to_gpx};
use vana::ride::log::{Entry, Kind, SLOT_SIZE, Sample, Slot, decode};

pub struct Ride {
    pub id: u32,
    pub entries: Vec<Entry>,
}

impl Ride {
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
                Some((from_e7(lat, lon), s.utc_ms.map(|ms| ms / 1000)))
            })
            .collect();
        to_gpx(&format!("VANA ride {}", self.id), &points)
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
            Some(ride) if ride.id == entry.ride_id => ride.entries.push(entry),
            _ => rides.push(Ride {
                id: entry.ride_id,
                entries: vec![entry],
            }),
        }
    }
    rides
}
