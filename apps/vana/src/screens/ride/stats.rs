//! The ride totals page is the built-in `RideStats` (`compose::builtin`).
//! Here is the time-in-zone bar the summary still draws by hand.
use super::zones::{BAR_WIDTH, Bar};
use crate::compose::tiles::zones::segments;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, Rect};
use crate::screens::scaled::Scaled;

/// A caption at `caption_left`, then across `rect` one segment per zone
/// ridden (`zones::segments`).
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
    let ends = bar.map(|ends| ends.map(|end| end as u16));
    let ends = ends.as_ref().map_or(&[][..], |ends| &ends[..]);
    segments(canvas, at, rect, ends, BAR_WIDTH as u16);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{balanced, prepared};
    use crate::compose::builtin::ScreenId;
    use crate::compose::data::Shares;
    use crate::ride::log::Sample;
    use crate::ride::zones::ZoneTime;
    use crate::screens::canvas::Shape;
    use crate::screens::chrome::{HEART, POWER, SPEED, TERTIARY, WHITE, zone_color};
    use crate::screens::ride::RidePage;
    use crate::screens::workout::{Page, View};

    fn stats(minutes: u64, sample: &Sample) -> View<'static> {
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Stats;
        view.radar_paired = true;
        for _ in 0..minutes * 60 {
            view.totals
                .add(1000, sample.speed_mm_s.unwrap_or(0).into(), sample, None);
            view.zone_time.add(1000, sample, &view.profile);
        }
        prepared(view)
    }

    /// The bars' segments: each one's row, width and colour.
    fn bars(view: &View) -> std::vec::Vec<(usize, usize, u16)> {
        view.canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, color, .. } => Some((rect.top, rect.width(), *color)),
                _ => None,
            })
            .collect()
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
        assert_eq!(color("0.93"), Some(POWER), "IF at the default FTP of 215 W");
        assert_eq!(color("14"), Some(POWER), "TSS");
        // Ten minutes at 200 W and 140 bpm: all Z4 power, all Z1 heart rate,
        // each one solid bar as wide as the other.
        let bars = bars(&view);
        assert_eq!(bars.len(), 2, "{bars:?}");
        assert_eq!((bars[0].2, bars[1].2), (zone_color(4), zone_color(1)));
        assert_eq!(bars[0].1, bars[1].1);
        assert!(bars[0].0 < bars[1].0);
        for caption in ["DIST KM", "MOVING", "AVG KM/H", "MAX KM/H", "AVG W"] {
            assert!(canvas.shows(caption), "{caption}");
        }
        for caption in ["NP W", "IF", "TSS", "AVG BPM", "ASCENT M", "PWR", "HR"] {
            assert!(canvas.shows(caption), "{caption}");
        }
    }

    #[test]
    fn mixed_zones_split_with_gaps_and_no_time_is_a_dim_track() {
        let empty = stats(0, &Sample::default());
        let tracks = bars(&empty);
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|(_, _, color)| *color == TERTIARY));
        let mut view = View::new(Page::Ride, 0, None);
        view.ride_page = RidePage::Stats;
        view.radar_paired = true;
        let mut time = ZoneTime::default();
        for watts in [100, 200, 300] {
            let sample = Sample {
                power_watts: Some(watts),
                ..Sample::default()
            };
            time.add(60_000, &sample, &view.profile);
        }
        view.zone_time = time;
        let view = prepared(view);
        let at = &view.layout.scaled;
        let power_row = bars(&view)[0].0;
        let segments: std::vec::Vec<_> = view
            .canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, .. } if rect.top == power_row => Some(rect),
                _ => None,
            })
            .collect();
        assert_eq!(segments.len(), 3);
        for pair in segments.windows(2) {
            assert_eq!(pair[1].left - pair[0].right, at.unit(2));
        }
        let track = tracks[0];
        assert_eq!(segments[2].right - segments[0].left, track.1);
    }

    #[test]
    fn a_zone_share_is_thousandths_of_the_ride() {
        let mut view = View::new(Page::Ride, 0, None);
        for (watts, minutes) in [(100, 1), (300, 3)] {
            let sample = Sample {
                power_watts: Some(watts),
                ..Sample::default()
            };
            view.zone_time.add(minutes * 60_000, &sample, &view.profile);
        }
        let shares =
            crate::compose::data::Data::zone_time(&view, crate::compose::data::Effort::Power);
        let Some(Shares(ends)) = shares else {
            panic!("time was ridden");
        };
        assert_eq!(ends[ends.len() - 1], Shares::WHOLE);
        assert!(ends.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(ends.contains(&250), "{ends:?}");
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
            stats(minutes, &sample).prepare().check_layout();
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
                prepared(view).prepare().check_layout();
            }
        }
    }

    #[test]
    fn the_stats_page_s_gaps_are_even_on_every_panel_and_variant() {
        balanced(ScreenId::RideStats);
    }
}
