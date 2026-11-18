//! The lap page: the open lap's time, distance and averages, the last
//! finished lap with how this one compares, and a bar per lap below.
//!
//! A difference carries its sign: green and "+" for faster or harder, plain
//! `SECONDARY` and "-" for slower or easier. An easy lap is not an error, so
//! it is never red.
use super::{cell, kilometres, lap_time, speed, whole};
use crate::ride::laps::{HISTORY, Lap};
use crate::screens::Layout;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    ACCENT, Align, BLACK, GREEN, HEART, POWER, Rect, SECONDARY, SPEED, WHITE, mix,
};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::{Text, View};
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The lap time's caption and the lap number share this baseline.
    pub head: usize,
    /// The lap time's baseline, under its caption.
    pub time: usize,
    /// Four bands of cells.
    pub bands: [Rect; 4],
    /// Lap bars stand on the screen's edge, in the toast band: a toast
    /// covers them for a moment.
    pub chart: Rect,
}

/// Rows the last band keeps under its value's baseline.
const UNDER_LAST_BAND: u16 = 8;

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Cap, Fr, Unit};
    let frame = super::frame(at);
    let pitch = super::CELL_PITCH as u16;
    let [head, _, time, _, cells, chart] = frame.body.rows(
        [
            Unit(16),
            Unit(4),
            Cap(FaceRole::Large),
            Unit(9),
            Unit(3 * pitch + super::VALUE_BASELINE + UNDER_LAST_BAND),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let bands = cells.rows([Unit(pitch), Unit(pitch), Unit(pitch), Fr(1)], Unit(0), at);
    Regions {
        head: head.bottom,
        time: time.bottom,
        bands,
        chart: chart.with_columns(frame.text),
    }
}

/// Each lap gets an equal slot of the width; its bar is centred in it.
const BAR_GAP: usize = 3;
const MAX_BAR_WIDTH: usize = 24;
/// The open lap and the finished laps kept.
const BARS: usize = HISTORY + 1;
const _: () = {
    let chart = regions(&crate::screens::scaled::BASE).chart;
    assert!(chart.width() / BARS > BAR_GAP + 4, "every lap's bar shows");
    assert!(chart.height() >= 24);
};

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

pub(super) fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.lap);
    let text_box = layout.ride.text;
    let (left, right) = (text_box.left, text_box.right);
    let mut canvas = Canvas::default();
    let lap = view.lap();
    let width = right - left;
    let mut text = Text::<12>::new();
    let _ = write!(text, "LAP {}", lap.number);
    let number = canvas.fit(
        &[at.face(FaceRole::Body)],
        text.as_bytes(),
        width,
        right,
        regions.head,
        Align::Right,
        ACCENT,
    );
    canvas.caption(
        at,
        b"LAP TIME",
        number.left - left,
        left,
        regions.head,
        Align::Left,
    );
    let mut time = Text::<12>::new();
    lap_time(&mut time, lap.time_ms / 1000);
    canvas.fit(
        &[at.face(FaceRole::Large), at.face(FaceRole::Value)],
        time.as_bytes(),
        width,
        left,
        regions.time,
        Align::Left,
        WHITE,
    );

    let value = |write: &dyn Fn(&mut Text<12>)| {
        let mut value = Text::<12>::new();
        write(&mut value);
        value
    };
    let band = |index: usize| regions.bands[index].top;
    let distance = value(&|v| kilometres(v, lap.distance_mm));
    cell(
        &mut canvas,
        layout,
        0,
        band(0),
        b"LAP KM",
        distance.as_bytes(),
        WHITE,
    );
    let power = value(&|v| whole(v, lap.avg_power()));
    cell(
        &mut canvas,
        layout,
        1,
        band(0),
        b"LAP W",
        power.as_bytes(),
        POWER,
    );
    let pace = value(&|v| speed(v, lap.avg_speed_mm_s()));
    cell(
        &mut canvas,
        layout,
        0,
        band(1),
        b"LAP KM/H",
        pace.as_bytes(),
        SPEED,
    );
    let heart = value(&|v| whole(v, lap.avg_heart_bpm()));
    cell(
        &mut canvas,
        layout,
        1,
        band(1),
        b"LAP BPM",
        heart.as_bytes(),
        HEART,
    );

    // The last finished lap by its number, as the toast named it.
    let previous = view.previous_lap();
    let mut name = Text::<12>::new();
    let mut watts_name = Text::<12>::new();
    let _ = match previous {
        Some(previous) => write!(name, "LAP {}", previous.number)
            .and_then(|()| write!(watts_name, "LAP {} W", previous.number)),
        None => name
            .push_str("PREV LAP")
            .and_then(|()| watts_name.push_str("PREV W")),
    };
    let dashes = || value(&|v| whole(v, None::<u8>));
    let (time, watts, faster, harder) = match previous {
        None => (dashes(), dashes(), dashes(), dashes()),
        Some(previous) => (
            value(&|v| lap_time(v, previous.time_ms / 1000)),
            value(&|v| whole(v, previous.avg_power())),
            value(&|v| speed_delta(v, lap.avg_speed_mm_s(), previous.avg_speed_mm_s())),
            value(&|v| power_delta(v, lap.avg_power(), previous.avg_power())),
        ),
    };
    cell(
        &mut canvas,
        layout,
        0,
        band(2),
        name.as_bytes(),
        time.as_bytes(),
        WHITE,
    );
    cell(
        &mut canvas,
        layout,
        1,
        band(2),
        watts_name.as_bytes(),
        watts.as_bytes(),
        POWER,
    );
    let color = delta_color(&faster);
    cell(
        &mut canvas,
        layout,
        0,
        band(3),
        b"KM/H DIFF",
        faster.as_bytes(),
        color,
    );
    let color = delta_color(&harder);
    cell(
        &mut canvas,
        layout,
        1,
        band(3),
        b"W DIFF",
        harder.as_bytes(),
        color,
    );

    chart(&mut canvas, layout, view.lap_bars(), lap.into());
    canvas
}

/// One bar per lap across the full width, oldest on the left and the open
/// lap on the right, its height the lap's average power (or speed on a ride
/// without power). Finished laps are dim, the open lap is in full colour,
/// and the best gets a white cap.
fn chart(canvas: &mut Canvas, layout: &Layout, finished: &[LapBar], open: LapBar) {
    let (at, area) = (&layout.scaled, layout.lap.chart);
    let with_power = finished
        .iter()
        .chain([&open])
        .any(|bar| bar.power.is_some());
    let (metric, color): (fn(&LapBar) -> u32, u16) = if with_power {
        (|bar| bar.power.map_or(0, u32::from), POWER)
    } else {
        (|bar| bar.speed_mm_s.unwrap_or(0), SPEED)
    };
    let bars = finished.iter().chain([&open]);
    let best = bars.clone().map(metric).max().unwrap_or(0);
    if best == 0 {
        return;
    }
    let count = finished.len() + 1;
    let mut capped = false;
    for (index, bar) in bars.enumerate() {
        let amount = metric(bar);
        if amount == 0 {
            continue;
        }
        let height = (amount as usize * area.height() / best as usize).max(at.unit(2));
        let (left, width) = slot(at, area, index, count);
        let rect = Rect {
            left,
            top: area.bottom - height,
            right: left + width,
            bottom: area.bottom,
        };
        let open = index + 1 == count;
        let shade = if open { color } else { mix(color, BLACK, 4) };
        canvas.shape(Shape::Bar {
            rect,
            radius: at.unit(2),
            color: shade,
        });
        if amount == best && !capped && count > 1 {
            capped = true;
            canvas.shape(Shape::Bar {
                rect: Rect {
                    bottom: rect.top + at.unit(2),
                    ..rect
                },
                radius: at.unit(1),
                color: WHITE,
            });
        }
    }
}

/// Lap `index` of `count`: its bar's left column and width, centred in an
/// equal share of the chart's width.
fn slot(at: &Scaled, area: Rect, index: usize, count: usize) -> (usize, usize) {
    let span = area.width();
    let (start, end) = (span * index / count, span * (index + 1) / count);
    let width = (end - start - at.unit(BAR_GAP)).min(at.unit(MAX_BAR_WIDTH));
    (area.left + start + (end - start - width) / 2, width)
}

/// The speed difference in tenths of km/h, truncated per side as the speed
/// readings are, or "--".
fn speed_delta(value: &mut Text<12>, now: Option<u32>, before: Option<u32>) {
    let tenths = |mm_s: u32| i64::from(mm_s) * 36 / 1000;
    match (now, before) {
        (Some(now), Some(before)) => signed(value, tenths(now) - tenths(before), |v, m| {
            write!(v, "{}.{}", m / 10, m % 10)
        }),
        _ => whole(value, None::<u8>),
    }
}

fn power_delta(value: &mut Text<12>, now: Option<u16>, before: Option<u16>) {
    match (now, before) {
        (Some(now), Some(before)) => signed(value, i64::from(now) - i64::from(before), |v, m| {
            write!(v, "{m}")
        }),
        _ => whole(value, None::<u8>),
    }
}

/// `delta` with a leading "+" or "-", and no sign for zero.
fn signed(
    value: &mut Text<12>,
    delta: i64,
    magnitude: impl FnOnce(&mut Text<12>, u64) -> core::fmt::Result,
) {
    let _ = value.push_str(match delta {
        1.. => "+",
        ..0 => "-",
        0 => "",
    });
    let _ = magnitude(value, delta.unsigned_abs());
}

/// Green for a gain; `SECONDARY` for a loss, no change or no reading.
fn delta_color(value: &Text<12>) -> u16 {
    match value.as_bytes().first() {
        Some(b'+') => GREEN,
        _ => SECONDARY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::laps::Laps;
    use crate::ride::log::Sample;
    use crate::screens::ride::{BODY as PAGE, RidePage};
    use crate::screens::workout::Page;

    const AT: &Scaled = &crate::screens::scaled::BASE;
    const CHART: Rect = regions(AT).chart;
    const CHART_HEIGHT: usize = CHART.height();
    const LEFT: usize = CHART.left;
    const RIGHT: usize = CHART.right;

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
        view.prepare().check_layout();
        view
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
                view.canvas()
                    .assert_laid_out(layout.ride.body, &["LAP TIME"], "long laps");
            }
        }
    }

    fn bars(view: &View) -> std::vec::Vec<(usize, usize, u16)> {
        view.canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, color, .. } => {
                    if *color != WHITE {
                        assert_eq!(
                            rect.bottom,
                            super::super::BODY.bottom,
                            "bars stand on the edge"
                        );
                    }
                    Some((rect.left, rect.height(), *color))
                }
                _ => None,
            })
            .collect()
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
        assert!(bars(&view).is_empty(), "nothing ridden, nothing charted");
        let view = ridden(&[], (60, steady(Some(200), 8334, 150)));
        let one = slot(AT, CHART, 0, 1);
        assert_eq!(bars(&view), [(one.0, CHART_HEIGHT, POWER)]);
        assert_eq!(one, ((LEFT + RIGHT - MAX_BAR_WIDTH) / 2, MAX_BAR_WIDTH));
    }

    #[test]
    fn laps_chart_across_the_width_with_the_best_capped() {
        let laps = [
            (60, steady(Some(200), 8334, 150)),
            (60, steady(Some(300), 8334, 150)),
            (60, steady(Some(150), 8334, 150)),
        ];
        let view = ridden(&laps, (60, steady(Some(240), 8334, 150)));
        let dim = mix(POWER, BLACK, 4);
        let lefts = |index: usize| slot(AT, CHART, index, 4).0;
        assert_eq!(
            bars(&view),
            [
                (lefts(0), CHART_HEIGHT * 200 / 300, dim),
                (lefts(1), CHART_HEIGHT, dim),
                (lefts(1), 2, WHITE),
                (lefts(2), CHART_HEIGHT / 2, dim),
                (lefts(3), CHART_HEIGHT * 240 / 300, POWER),
            ]
        );
        assert!(
            lefts(0) - LEFT < MAX_BAR_WIDTH && RIGHT - (lefts(3) + MAX_BAR_WIDTH) < MAX_BAR_WIDTH,
            "the bars span the page's width"
        );
        // Seventeen bars still keep their gaps and stay inside the margins.
        for count in 1..=BARS {
            let mut right = LEFT;
            for index in 0..count {
                let (left, width) = slot(AT, CHART, index, count);
                assert!(left >= right && width >= 4, "{index} of {count}");
                right = left + width + BAR_GAP;
            }
            assert!(right - BAR_GAP <= RIGHT);
        }
        // Without power, the bars show speed.
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
            view.canvas().assert_laid_out(
                PAGE,
                &[
                    "LAP TIME",
                    "LAP 17",
                    "LAP 16",
                    "LAP 16 W",
                    "KM/H DIFF",
                    "W DIFF",
                ],
                &std::format!("{}s open", open.0),
            );
            assert!(bars(&view).len() > HISTORY, "every lap and the cap");
        }
    }
}
