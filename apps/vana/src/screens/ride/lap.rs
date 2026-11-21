//! The lap page is the built-in `RideLap` (`compose::builtin`): the open
//! lap's time, distance and averages, the last finished lap with how this
//! one compares, and a bar per lap. Here is what a lap's bar shows.
use crate::ride::laps::{HISTORY, Lap};

/// What a lap's bar shows: its average power, or its speed without power.
#[derive(Clone, Copy, Debug, Default)]
pub struct LapBar {
    pub power: Option<u16>,
    pub speed_mm_s: Option<u32>,
}

impl From<Lap> for LapBar {
    fn from(lap: Lap) -> Self {
        Self {
            power: lap.avg_power(),
            speed_mm_s: lap.avg_speed_mm_s(),
        }
    }
}

/// Finished laps, oldest first, as the lap page charts them.
pub type LapBars = heapless::Vec<LapBar, HISTORY>;

#[cfg(test)]
mod tests {
    use super::super::tests::{balanced, prepared};
    use super::*;
    use crate::compose::builtin::ScreenId;
    use crate::ride::laps::Laps;
    use crate::ride::log::Sample;
    use crate::screens::canvas::Shape;
    use crate::screens::chrome::{BLACK, GREEN, POWER, SECONDARY, WHITE, mix};
    use crate::screens::ride::RidePage;
    use crate::screens::workout::{Page, View};

    fn steady(watts: Option<u16>, speed_mm_s: u32, bpm: u16) -> Sample {
        Sample {
            speed_mm_s: Some(speed_mm_s),
            power_watts: watts,
            heart_bpm: Some(bpm),
            ..Sample::default()
        }
    }

    /// `laps` finished laps of `seconds` each, then the open lap's.
    fn ridden(laps: &[(u64, Sample)], open: (u64, Sample)) -> View<'static> {
        ridden_on(&crate::screens::BASE, true, laps, open)
    }

    fn ridden_on(
        layout: &'static crate::screens::Layout,
        radar: bool,
        laps: &[(u64, Sample)],
        open: (u64, Sample),
    ) -> View<'static> {
        let mut ride = Laps::default();
        let mut now = 0;
        for (index, &(seconds, sample)) in laps.iter().chain([&open]).enumerate() {
            // One step per lap, kept under the automatic lap distance.
            let speed = u64::from(sample.speed_mm_s.unwrap_or(0));
            let distance = (speed * seconds).min(4_900_000);
            assert!(ride.add(seconds * 1000, distance, &sample, now).is_none());
            now += seconds * 1000;
            if index < laps.len() {
                let _ = ride.lap(now);
            }
        }
        let mut view = View::new(Page::Ride, 0, None);
        view.layout = layout;
        view.ride_page = RidePage::Lap;
        view.radar_paired = radar;
        let kept = (0..HISTORY).rev().filter_map(|back| ride.finished(back));
        view.show_laps(
            kept.map(LapBar::from).collect(),
            ride.current(),
            ride.previous(),
        );
        let mut view = prepared(view);
        view.prepare().check_layout();
        view
    }

    /// Each bar's left, height and colour.
    fn bars(view: &View) -> std::vec::Vec<(usize, usize, u16)> {
        let bottom = view.layout.scaled.panel.height;
        view.canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, color, .. } => {
                    if *color != WHITE {
                        assert_eq!(rect.bottom, bottom, "bars stand on the edge");
                    }
                    Some((rect.left, rect.height(), *color))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn long_laps_fit_every_panel() {
        let fast = steady(Some(1999), 33_333, 220);
        let slow = steady(Some(1), 300, 40);
        let mut laps = std::vec![(35_999, fast); HISTORY];
        laps[0] = (60, slow);
        for layout in crate::screens::panels::ALL {
            for radar in [false, true] {
                let view = ridden_on(layout, radar, &laps, (359_999, fast));
                assert!(view.canvas().shows("LAP TIME"));
                assert!(view.canvas().shows("99:59:59"));
            }
        }
    }

    #[test]
    fn differences_are_signed_and_coloured() {
        let first = (300, steady(Some(200), 8334, 150));
        let color = |view: &View, text| view.canvas().find(text).map(|run| run.color);
        let harder = ridden(&[first], (125, steady(Some(250), 9000, 150)));
        assert!(harder.canvas().shows("LAP 2"));
        assert!(harder.canvas().shows("LAP 1"));
        assert!(harder.canvas().shows("LAP 1 W"));
        assert_eq!(color(&harder, "02:05"), Some(WHITE));
        assert_eq!(color(&harder, "05:00"), Some(WHITE));
        assert_eq!(color(&harder, "+2.4"), Some(GREEN));
        assert_eq!(color(&harder, "+50"), Some(GREEN));
        assert_eq!(color(&harder, "250"), Some(POWER));
        let easier = ridden(&[first], (60, steady(Some(150), 7667, 150)));
        assert_eq!(color(&easier, "-2.4"), Some(SECONDARY));
        assert_eq!(color(&easier, "-50"), Some(SECONDARY));
        let level = ridden(&[first], (60, steady(Some(200), 8334, 150)));
        assert_eq!(color(&level, "0.0"), Some(SECONDARY));
        assert_eq!(color(&level, "0"), Some(SECONDARY));
    }

    #[test]
    fn the_first_lap_has_no_comparison_and_one_bar() {
        let view = ridden(&[], (0, Sample::default()));
        assert!(view.canvas().shows("LAP 1"));
        assert!(view.canvas().shows("00:00"));
        assert!(view.canvas().shows("PREV LAP"));
        assert!(view.canvas().shows("PREV W"));
        assert!(bars(&view).is_empty(), "nothing ridden, nothing charted");
        let view = ridden(&[], (60, steady(Some(200), 8334, 150)));
        let shown = bars(&view);
        assert_eq!(shown.len(), 1, "one bar and no cap");
        assert_eq!(shown[0].2, POWER);
    }

    #[test]
    fn finished_laps_are_dim_and_the_best_is_capped() {
        let laps = [
            (60, steady(Some(200), 8334, 150)),
            (60, steady(Some(300), 8334, 150)),
            (60, steady(Some(150), 8334, 150)),
        ];
        let view = ridden(&laps, (60, steady(Some(240), 8334, 150)));
        let dim = mix(POWER, BLACK, 4);
        let colours: std::vec::Vec<u16> = bars(&view).iter().map(|bar| bar.2).collect();
        assert_eq!(colours, [dim, dim, WHITE, dim, POWER]);
        let laps = [(60, steady(None, 8334, 150))];
        let view = ridden(&laps, (60, steady(None, 9000, 150)));
        assert!(bars(&view).iter().all(|&(_, _, color)| color != POWER));
    }

    /// Sixteen kept laps and the widest readings still lay out whole.
    #[test]
    fn extreme_laps_never_overlap_or_clip() {
        let flat_out = steady(Some(1999), 33_333, 220);
        let laps = [(3600, flat_out); HISTORY];
        for open in [
            (0, Sample::default()),
            (359_999, flat_out),
            (5, steady(Some(1), 300, 40)),
        ] {
            let view = ridden(&laps, open);
            for shown in [
                "LAP TIME",
                "LAP 17",
                "LAP 16",
                "LAP 16 W",
                "KM/H DIFF",
                "W DIFF",
            ] {
                assert!(view.canvas().shows(shown), "{shown}, {}s open", open.0);
            }
            assert!(bars(&view).len() > HISTORY, "every lap and the cap");
        }
    }

    #[test]
    fn the_lap_page_s_gaps_are_even_on_every_panel_and_variant() {
        balanced(ScreenId::RideLap);
    }
}
