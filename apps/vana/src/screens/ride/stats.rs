//! The ride totals page: what this ride has added up so far, kept after stop,
//! above bars of the time spent in each power and heart rate zone.
use super::zones::{BAR_WIDTH, Bar, COLOURS};
use super::{CELL_PITCH, LEFT, RIGHT, cell, kilometres, lap_time, speed, whole};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, CADENCE, GRADE, HEART, POWER, Rect, SPEED, TERTIARY, WHITE};
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::SPILL;

/// The first band's caption top; five bands of two cells follow. Work and
/// peak heart rate stay in the saved ride; the page keeps room between
/// readings instead.
const FIRST: usize = 25;
/// Time-in-zone bars, in the toast band: a toast covers them for a moment.
const POWER_BAR: usize = 286;
const HEART_BAR: usize = 302;
const BAR_HEIGHT: usize = 8;
/// "PWR" and "HR" sit left of the bars.
const BAR_LEFT: usize = RIGHT - BAR_WIDTH;
const _: () = assert!(BAR_LEFT >= LEFT + 36 + SPILL + 8, "PWR needs 36 columns");
const _: () = assert!(
    FIRST + 4 * CELL_PITCH + 41 + 8 < POWER_BAR,
    "the bars clear the cells"
);
/// Black between zone segments.
const SEGMENT_GAP: usize = 2;

/// Captions in reading order, two per band, with each value's colour.
const CELLS: [(&[u8], u16); 10] = [
    (b"DIST KM", WHITE),
    (b"MOVING", WHITE),
    (b"AVG KM/H", SPEED),
    (b"MAX KM/H", SPEED),
    (b"AVG W", POWER),
    (b"MAX W", POWER),
    (b"NP W", POWER),
    (b"AVG BPM", HEART),
    (b"AVG RPM", CADENCE),
    (b"ASCENT M", GRADE),
];

pub(super) fn prepare(view: &View) -> Canvas {
    let totals = &view.totals;
    let mut values: [Text<12>; 10] = core::array::from_fn(|_| Text::new());
    let [
        distance,
        moving,
        avg_speed,
        max_speed,
        avg_power,
        max_power,
        np,
        avg_heart,
        cadence,
        ascent,
    ] = &mut values;
    kilometres(distance, totals.distance_mm());
    lap_time(moving, totals.moving_ms() / 1000);
    speed(avg_speed, totals.avg_speed_mm_s());
    speed(max_speed, totals.max_speed_mm_s());
    whole(avg_power, totals.avg_power());
    whole(max_power, totals.max_power());
    whole(np, totals.normalized_power());
    whole(avg_heart, totals.avg_heart_bpm());
    whole(cadence, totals.avg_cadence_rpm());
    whole(ascent, Some((totals.ascent_dm() + 5) / 10));

    let mut canvas = Canvas::default();
    for (index, ((caption, color), value)) in CELLS.iter().zip(&values).enumerate() {
        let top = FIRST + index / 2 * CELL_PITCH;
        cell(
            &mut canvas,
            index % 2,
            top,
            caption,
            value.as_bytes(),
            *color,
        );
    }
    let marks = &view.marks;
    zone_bar(&mut canvas, b"PWR", POWER_BAR, marks.power_time);
    zone_bar(&mut canvas, b"HR", HEART_BAR, marks.heart_time);
    canvas
}

/// A caption, then one segment per zone ridden, in zone colours with black
/// between them; a `TERTIARY` track before any time is recorded.
fn zone_bar<const N: usize>(canvas: &mut Canvas, caption: &[u8], top: usize, bar: Bar<N>) {
    // The caption's capitals centre on the bar.
    canvas.caption(caption, BAR_LEFT - 8 - LEFT, LEFT, top + 10, Align::Left);
    let bar_rect = |left: usize, right: usize| Rect {
        left: BAR_LEFT + left,
        top,
        right: BAR_LEFT + right,
        bottom: top + BAR_HEIGHT,
    };
    let Some(ends) = bar else {
        canvas.shape(Shape::Bar {
            rect: bar_rect(0, BAR_WIDTH),
            radius: 2,
            color: TERTIARY,
        });
        return;
    };
    // The last zone with time ends the bar; no gap follows it.
    let last = (0..N)
        .rposition(|zone| ends[zone] > zone.checked_sub(1).map_or(0, |before| ends[before]))
        .unwrap_or(0);
    let mut start = 0;
    for (zone, &end) in ends.iter().enumerate() {
        let right = if zone == last {
            end
        } else {
            end.saturating_sub(SEGMENT_GAP)
        };
        if right > start {
            canvas.shape(Shape::Bar {
                rect: bar_rect(start, right),
                radius: 2,
                color: COLOURS[zone],
            });
        }
        start = start.max(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::Sample;
    use crate::ride::zones::ZoneTime;
    use crate::screens::chrome::zone_color;
    use crate::screens::ride::{BODY, RidePage};
    use crate::screens::workout::Page;

    fn stats(minutes: u64, sample: &Sample) -> View {
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Stats;
        view.radar_paired = true;
        for _ in 0..minutes * 60 {
            view.totals
                .add(1000, sample.speed_mm_s.unwrap_or(0).into(), sample, None);
            view.zone_time.add(1000, sample, &view.profile);
        }
        view.prepare();
        view
    }

    #[test]
    fn totals_take_their_metric_colours_and_zones_fill_the_bars() {
        let sample = Sample {
            speed_mm_s: Some(8333),
            power_watts: Some(200),
            heart_bpm: Some(140),
            cadence_tenths: Some(900),
            ..Sample::default()
        };
        let view = stats(10, &sample);
        let canvas = &view.canvas;
        let color = |text| canvas.find(text).map(|run| run.color);
        assert_eq!(color("5.00"), Some(WHITE));
        assert_eq!(color("10:00"), Some(WHITE));
        assert_eq!(color("29.9"), Some(SPEED));
        assert_eq!(color("200"), Some(POWER));
        assert_eq!(color("140"), Some(HEART));
        assert_eq!(color("90"), Some(CADENCE));
        // Ten minutes at 200 W and 140 bpm: all Z4 power, all Z1 heart rate,
        // each one solid bar.
        let bars: std::vec::Vec<_> = canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, color, .. } => Some((rect.top, rect.width(), *color)),
                _ => None,
            })
            .collect();
        assert_eq!(
            bars,
            [
                (POWER_BAR, BAR_WIDTH, zone_color(4)),
                (HEART_BAR, BAR_WIDTH, zone_color(1))
            ]
        );
        canvas.assert_laid_out(BODY, &["DIST KM", "ASCENT M", "PWR", "HR"], "stats");
    }

    #[test]
    fn mixed_zones_split_with_gaps_and_no_time_is_a_dim_track() {
        let empty = stats(0, &Sample::default());
        assert!(empty.canvas.shapes.iter().all(|shape| matches!(
            shape,
            Shape::Bar { color: TERTIARY, rect, .. } if rect.width() == BAR_WIDTH
        )));
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Stats;
        let mut time = ZoneTime::default();
        for watts in [100, 200, 300] {
            let sample = Sample {
                power_watts: Some(watts),
                ..Sample::default()
            };
            time.add(60_000, &sample, &view.profile);
        }
        view.zone_time = time;
        view.prepare();
        let segments: std::vec::Vec<_> = view
            .canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, .. } if rect.top == POWER_BAR => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(segments.len(), 3);
        for pair in segments.windows(2) {
            assert_eq!(pair[1].left - pair[0].right, SEGMENT_GAP);
        }
        assert_eq!(segments[2].right, RIGHT);
    }

    /// The widest totals a ride can reach still fit their cells whole.
    #[test]
    fn extreme_totals_never_overlap_or_clip() {
        let fast = Sample {
            speed_mm_s: Some(33_333),
            power_watts: Some(1999),
            heart_bpm: Some(220),
            cadence_tenths: Some(1800),
            ..Sample::default()
        };
        for (minutes, sample) in [(0, Sample::default()), (1, fast), (100, fast)] {
            let view = stats(minutes, &sample);
            view.canvas
                .assert_laid_out(BODY, &[], &std::format!("{minutes} min"));
        }
        // A long ride: 1000 km over 99:59:59 with a full ascent.
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Stats;
        for _ in 0..36 {
            view.totals
                .add(9_999_000, 1_000_000_000 / 36, &fast, Some(88_000));
        }
        view.prepare();
        view.canvas.assert_laid_out(BODY, &[], "long ride");
    }
}
