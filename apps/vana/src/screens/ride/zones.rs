//! Training-zone colours: the zone chips and gauges on the main page.
//! Everything is measured in `marks`, once per frame; drawing only compares
//! coordinates.
use crate::ride::zones::TrainingProfile;
use crate::screens::workout::{Graphics, View};

/// Pixels in a zone gauge.
pub(in crate::screens) const GAUGE_WIDTH: usize = 56;

/// A gliding reading's position on its zone gauge.
#[derive(Clone, Copy, Default, Hash)]
pub(in crate::screens) struct Mark {
    /// The zone it is in, from 1.
    pub(in crate::screens) zone: u8,
    /// Gauge pixels lit, 1..=GAUGE_WIDTH.
    pub(in crate::screens) fill: u8,
}

#[derive(Clone, Copy, Default)]
pub(in crate::screens) struct Marks {
    pub(super) power: Option<Mark>,
    pub(super) heart: Option<Mark>,
}

impl Marks {
    pub(in crate::screens) fn gauges(&self) -> [Option<Mark>; 2] {
        [self.power, self.heart]
    }
}

pub(in crate::screens) fn marks(view: &View) -> Marks {
    let [power, heart] = gauges(&view.graphics, &view.profile);
    Marks { power, heart }
}

/// The power and heart-rate gauges' marks for the gliding readings.
pub(in crate::screens) fn gauges(
    graphics: &Graphics,
    profile: &TrainingProfile,
) -> [Option<Mark>; 2] {
    [
        graphics.power_3s.map(|watts| {
            let zone = profile.power_zone(watts);
            mark(zone, u32::from(watts), profile.power_range(zone))
        }),
        graphics.heart_bpm.map(|bpm| {
            let zone = profile.heart_zone(bpm);
            mark(zone, u32::from(bpm), profile.heart_range(zone))
        }),
    ]
}

fn mark(zone: u8, value: u32, (low, high): (u32, u32)) -> Mark {
    let into = value.clamp(low, high) - low;
    Mark {
        zone,
        fill: (into as usize * GAUGE_WIDTH / (high - low) as usize).clamp(1, GAUGE_WIDTH) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauges_show_position_within_the_zone() {
        // Z4 spans 195..227 W at the default FTP.
        let profile = TrainingProfile::default();
        let at = |watts| usize::from(mark(4, watts, profile.power_range(4)).fill);
        assert_eq!(at(195), 1);
        assert_eq!(at(211), GAUGE_WIDTH / 2);
        assert_eq!(at(226), GAUGE_WIDTH * 31 / 32);
        assert_eq!(at(500), GAUGE_WIDTH);
        assert_eq!(
            usize::from(mark(7, 2000, profile.power_range(7)).fill),
            GAUGE_WIDTH
        );
        assert_eq!(mark(1, 0, profile.power_range(1)).fill, 1);
    }

    #[test]
    fn the_power_gauge_follows_three_second_power() {
        let mut view = View::new(crate::screens::workout::Page::Ride, 0, None);
        view.sample.power_watts = Some(500);
        view.graphics.power_3s = Some(211);
        let fill = marks(&view).power.map(|mark| usize::from(mark.fill));
        assert_eq!(fill, Some(GAUGE_WIDTH / 2), "Z4 half full, not Z7's 500 W");
        view.graphics.power_3s = None;
        assert!(marks(&view).power.is_none());
    }
}
