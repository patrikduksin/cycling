//! Training-zone colours: the zone chips and gauges on the main page and the
//! time-in-zone bars on the stats page. Everything is measured in `marks`,
//! once per frame; drawing only compares coordinates.
use super::GRID;
use crate::ride::zones::{HEART_ZONES, POWER_ZONES, TrainingProfile, ZoneTime};
use crate::screens::workout::View;
use core::ops::Range;
use firmware_shell::rendering::row::Row;

/// Z1 grey, Z2 blue, Z3 green, Z4 yellow, Z5 orange, Z6 red, Z7 purple. All
/// are light enough for black text; the zone number is always drawn too.
pub const COLOURS: [u16; POWER_ZONES] = [0x8410, 0x249f, 0x07e0, 0xffe0, 0xfc60, 0xf800, 0xa21f];

/// Pixels in a zone gauge.
pub(super) const GAUGE_WIDTH: usize = 56;
/// Pixels in a time-in-zone bar.
pub(super) const BAR_WIDTH: usize = 180;

/// A live reading's zone and its position within that zone.
#[derive(Clone, Copy, Default)]
pub(in crate::screens) struct Mark {
    pub(super) colour: u16,
    /// Gauge pixels lit, 1..=GAUGE_WIDTH.
    pub(super) fill: usize,
}

/// Where each zone's share of a bar ends, from the bar's left edge.
pub(super) type Bar<const N: usize> = Option<[usize; N]>;

#[derive(Clone, Copy, Default)]
pub(in crate::screens) struct Marks {
    pub(super) power: Option<Mark>,
    pub(super) heart: Option<Mark>,
    pub(super) power_time: Bar<POWER_ZONES>,
    pub(super) heart_time: Bar<HEART_ZONES>,
}

pub(in crate::screens) fn marks(view: &View) -> Marks {
    let profile: &TrainingProfile = &view.profile;
    let time: &ZoneTime = &view.zone_time;
    Marks {
        power: view.sample.power_watts.map(|watts| {
            let zone = profile.power_zone(watts);
            mark(zone, u32::from(watts), profile.power_range(zone))
        }),
        heart: view.sample.heart_bpm.map(|bpm| {
            let zone = profile.heart_zone(bpm);
            mark(zone, u32::from(bpm), profile.heart_range(zone))
        }),
        power_time: bar(&time.power_ms),
        heart_time: bar(&time.heart_ms),
    }
}

fn mark(zone: u8, value: u32, (low, high): (u32, u32)) -> Mark {
    let into = value.clamp(low, high) - low;
    Mark {
        colour: COLOURS[usize::from(zone - 1)],
        fill: (into as usize * GAUGE_WIDTH / (high - low) as usize).clamp(1, GAUGE_WIDTH),
    }
}

fn bar<const N: usize>(time: &[u64; N]) -> Bar<N> {
    let total: u64 = time.iter().sum();
    if total == 0 {
        return None;
    }
    let mut so_far = 0;
    Some(core::array::from_fn(|zone| {
        so_far += time[zone];
        (so_far * BAR_WIDTH as u64 / total) as usize
    }))
}

/// The gauge `left..left + GAUGE_WIDTH` over the columns `shape` covers.
pub(super) fn paint_gauge(row: &mut Row, mark: Mark, left: usize, shape: Range<usize>) {
    row.span(shape.clone(), GRID);
    row.span(shape.start..shape.end.min(left + mark.fill), mark.colour);
}

/// The bar `left..left + BAR_WIDTH` over the columns `shape` covers.
pub(super) fn paint_bar<const N: usize>(
    row: &mut Row,
    bar: Bar<N>,
    left: usize,
    shape: Range<usize>,
) {
    row.span(shape.clone(), GRID);
    let Some(ends) = bar else {
        return;
    };
    let mut start = left;
    for (zone, &end) in ends.iter().enumerate() {
        let end = left + end;
        row.span(start.max(shape.start)..end.min(shape.end), COLOURS[zone]);
        start = start.max(end);
    }
}

/// A gauge `left..left + GAUGE_WIDTH`: lit in the zone colour up to the
/// reading, then a dim track.
#[cfg(test)]
pub(super) fn gauge_pixel(mark: Mark, left: usize, x: usize) -> u16 {
    if x < left + mark.fill {
        mark.colour
    } else {
        GRID
    }
}

/// A bar `left..left + BAR_WIDTH` split by time in each zone, or a dim track
/// before any time is recorded.
#[cfg(test)]
pub(super) fn bar_pixel<const N: usize>(bar: Bar<N>, left: usize, x: usize) -> u16 {
    let Some(ends) = bar else {
        return GRID;
    };
    let offset = x - left;
    ends.iter()
        .position(|&end| offset < end)
        .map_or(GRID, |zone| COLOURS[zone])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauges_show_position_within_the_zone() {
        // Z4 spans 195..227 W at the default FTP.
        let profile = TrainingProfile::default();
        let at = |watts| mark(4, watts, profile.power_range(4)).fill;
        assert_eq!(at(195), 1);
        assert_eq!(at(211), GAUGE_WIDTH / 2);
        assert_eq!(at(226), GAUGE_WIDTH * 31 / 32);
        assert_eq!(at(500), GAUGE_WIDTH);
        assert_eq!(mark(7, 2000, profile.power_range(7)).fill, GAUGE_WIDTH);
        assert_eq!(mark(1, 0, profile.power_range(1)).fill, 1);
    }

    #[test]
    fn bars_split_by_time_and_stay_dim_when_empty() {
        assert!(bar(&[0u64; 5]).is_none());
        let ends = bar(&[0, 1000, 0, 3000, 0]).unwrap();
        assert_eq!(ends, [0, 45, 45, 180, 180]);
        assert_eq!(bar_pixel(Some(ends), 10, 10), COLOURS[1]);
        assert_eq!(bar_pixel(Some(ends), 10, 54), COLOURS[1]);
        assert_eq!(bar_pixel(Some(ends), 10, 55), COLOURS[3]);
        assert_eq!(bar_pixel(Some(ends), 10, 189), COLOURS[3]);
        assert_eq!(bar_pixel(None::<[usize; 5]>, 10, 50), GRID);
    }
}
