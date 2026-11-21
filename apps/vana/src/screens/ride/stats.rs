//! The ride totals page: what this ride has added up so far, kept after stop,
//! above bars of the time spent in each power and heart rate zone.
use super::zones::{BAR_WIDTH, Bar, COLOURS};
use super::{cell, kilometres, lap_time, speed, whole};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, GRADE, HEART, POWER, Rect, SPEED, TERTIARY, WHITE};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::{FaceRole, SPILL};

/// The stats page on one panel: five bands of two cells, then the
/// time-in-zone bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub bands: [Rect; 5],
    /// Time-in-zone bars, in the toast band: a toast covers them for a
    /// moment. "PWR" and "HR" sit left of them.
    pub power_bar: Rect,
    pub heart_bar: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let frame = super::frame(at);
    let [_, cells, _, power, _, heart, _] = frame.body.rows(
        [
            Unit(1),
            Unit(5 * super::CELL_PITCH as u16),
            Fr(1),
            Unit(BAR_HEIGHT),
            Unit(8),
            Unit(BAR_HEIGHT),
            Unit(10),
        ],
        Unit(0),
        at,
    );
    let pitch = Unit(super::CELL_PITCH as u16);
    let bands = cells.rows([pitch; 5], Unit(0), at);
    let caption = at.face(FaceRole::Caption).width(b"PWR") + SPILL;
    let bar = Rect {
        left: frame.text.left + caption + at.unit(8),
        ..frame.text
    };
    Regions {
        bands,
        power_bar: power.with_columns(bar),
        heart_bar: heart.with_columns(bar),
    }
}

const BAR_HEIGHT: u16 = 8;
/// Black between zone segments.
const SEGMENT_GAP: usize = 2;

/// Captions in reading order, two per band, with each value's colour.
const CELLS: [(&[u8], u16); 10] = [
    (b"DIST KM", WHITE),
    (b"MOVING", WHITE),
    (b"AVG KM/H", SPEED),
    (b"MAX KM/H", SPEED),
    (b"AVG W", POWER),
    (b"NP W", POWER),
    (b"IF", POWER),
    (b"TSS", POWER),
    (b"AVG BPM", HEART),
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
        np,
        intensity,
        stress,
        avg_heart,
        ascent,
    ] = &mut values;
    kilometres(distance, totals.distance_mm());
    lap_time(moving, totals.moving_ms() / 1000);
    speed(avg_speed, totals.avg_speed_mm_s());
    speed(max_speed, totals.max_speed_mm_s());
    whole(avg_power, totals.avg_power());
    whole(np, totals.normalized_power());
    let ftp = view.profile.ftp_watts;
    hundredths(intensity, totals.intensity_hundredths(ftp));
    whole(stress, totals.stress_score(ftp));
    whole(avg_heart, totals.avg_heart_bpm());
    whole(ascent, Some((totals.ascent_dm() + 5) / 10));

    let layout = view.layout;
    let regions = &layout.stats;
    let mut canvas = Canvas::default();
    for (index, ((caption, color), value)) in CELLS.iter().zip(&values).enumerate() {
        let top = regions.bands[index / 2].top;
        cell(
            &mut canvas,
            layout,
            index % 2,
            top,
            caption,
            value.as_bytes(),
            *color,
        );
    }
    let marks = &view.marks;
    let left = layout.ride.text.left;
    zone_bar(
        &mut canvas,
        &layout.scaled,
        b"PWR",
        left,
        regions.power_bar,
        marks.power_time,
    );
    zone_bar(
        &mut canvas,
        &layout.scaled,
        b"HR",
        left,
        regions.heart_bar,
        marks.heart_time,
    );
    canvas
}

/// "1.23", or "--" without a value.
fn hundredths(value: &mut Text<12>, reading: Option<u16>) {
    use core::fmt::Write;
    let _ = match reading {
        Some(v) => write!(value, "{}.{:02}", v / 100, v % 100),
        None => value.push_str("--"),
    };
}

/// A caption at `caption_left`, then across `rect` one segment per zone
/// ridden, in zone colours with black between them; a `TERTIARY` track
/// before any time is recorded.
pub(super) fn zone_bar<const N: usize>(
    canvas: &mut Canvas,
    at: &Scaled,
    caption: &[u8],
    caption_left: usize,
    rect: Rect,
    bar: Bar<N>,
) {
    // The caption's capitals centre on the bar.
    canvas.caption(
        at,
        caption,
        rect.left - at.unit(8) - caption_left,
        caption_left,
        rect.top + at.unit(10),
        Align::Left,
    );
    let radius = at.unit(2);
    let bar_rect = |left: usize, right: usize| Rect {
        left: rect.left + left,
        right: rect.left + right,
        ..rect
    };
    let Some(ends) = bar else {
        canvas.shape(Shape::Bar {
            rect,
            radius,
            color: TERTIARY,
        });
        return;
    };
    let ends = ends.map(|end| end * rect.width() / BAR_WIDTH);
    let gap = at.unit(SEGMENT_GAP);
    // The last zone with time ends the bar; no gap follows it.
    let last = (0..N)
        .rposition(|zone| ends[zone] > zone.checked_sub(1).map_or(0, |before| ends[before]))
        .unwrap_or(0);
    let mut start = 0;
    for (zone, &end) in ends.iter().enumerate() {
        let right = if zone == last {
            end
        } else {
            end.saturating_sub(gap)
        };
        if right > start {
            canvas.shape(Shape::Bar {
                rect: bar_rect(start, right),
                radius,
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

    const REGIONS: Regions = crate::screens::BASE.stats;

    fn stats(minutes: u64, sample: &Sample) -> View<'static> {
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
        let canvas = view.canvas();
        let color = |text| canvas.find(text).map(|run| run.color);
        assert_eq!(color("5.00"), Some(WHITE));
        assert_eq!(color("10:00"), Some(WHITE));
        assert_eq!(color("29.9"), Some(SPEED));
        assert_eq!(color("200"), Some(POWER));
        assert_eq!(color("140"), Some(HEART));
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
                (REGIONS.power_bar.top, BAR_WIDTH, zone_color(4)),
                (REGIONS.heart_bar.top, BAR_WIDTH, zone_color(1))
            ]
        );
        assert_eq!(color("0.93"), Some(POWER), "IF at the default FTP of 215 W");
        assert_eq!(color("14"), Some(POWER), "TSS");
        canvas.assert_laid_out(BODY, &["DIST KM", "ASCENT M", "PWR", "HR"], "stats");
    }

    #[test]
    fn mixed_zones_split_with_gaps_and_no_time_is_a_dim_track() {
        let empty = stats(0, &Sample::default());
        assert!(empty.canvas().shapes.iter().all(|shape| matches!(
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
            .canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, .. } if rect.top == REGIONS.power_bar.top => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(segments.len(), 3);
        for pair in segments.windows(2) {
            assert_eq!(pair[1].left - pair[0].right, SEGMENT_GAP);
        }
        assert_eq!(segments[2].right, REGIONS.power_bar.right);
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
            view.canvas()
                .assert_laid_out(BODY, &[], &std::format!("{minutes} min"));
        }
        for layout in crate::screens::panels::ALL {
            for radar in [false, true] {
                let mut view = View::new(Page::Ride, 0, None);
                view.layout = layout;
                view.ride_page = RidePage::Stats;
                view.radar_paired = radar;
                for _ in 0..36 {
                    view.totals
                        .add(9_999_000, 1_000_000_000 / 36, &fast, Some(88_000));
                }
                view.prepare().check_layout();
                let body = layout.ride.body;
                view.canvas().assert_laid_out(body, &[], "long ride");
            }
        }
    }
}
