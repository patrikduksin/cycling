//! One bar per lap, oldest on the left and the open lap on the right, its
//! height the lap's average power, or its speed on a ride without power.
//! Finished laps are dim, the open lap in full colour, and the best gets a
//! white cap. The bars stand on the box's bottom edge.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::ride::laps::HISTORY;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{BLACK, POWER, Rect, SPEED, WHITE, mix};
use crate::screens::ride::LapBar;
use crate::screens::scaled::Scaled;

pub struct Laps;

const BAR_GAP_UNITS: usize = 3;
const MAX_BAR_UNITS: usize = 24;
const LEAST_BAR_UNITS: usize = 4;
const LEAST_HEIGHT_UNITS: usize = 24;
/// The open lap and the finished laps kept.
const BARS: usize = HISTORY + 1;

impl Tile for Laps {
    type Arg = ();
    const NAME: &'static str = "laps";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: (BARS * (at.unit(BAR_GAP_UNITS) + at.unit(LEAST_BAR_UNITS))) as u16,
            height: at.unit(LEAST_HEIGHT_UNITS) as u16,
            // A bar a lap and the best's cap.
            shapes: BARS as u8 + 1,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if let Some((finished, open)) = data.laps() {
            chart(out, at, placed.rect.rect(), finished, open);
        }
    }

    fn heavy() -> bool {
        true
    }
}

fn chart(out: &mut Canvas, at: &Scaled, area: Rect, finished: &[LapBar], open: LapBar) {
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
        out.shape(Shape::Bar {
            rect,
            radius: at.unit(2),
            color: shade,
        });
        if amount == best && !capped && count > 1 {
            capped = true;
            out.shape(Shape::Bar {
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
    let width = (end - start - at.unit(BAR_GAP_UNITS)).min(at.unit(MAX_BAR_UNITS));
    (area.left + start + (end - start - width) / 2, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: &Scaled = &crate::screens::scaled::BASE;
    const AREA: Rect = Rect {
        left: 10,
        top: 200,
        right: 210,
        bottom: 260,
    };

    fn bars(power: &[u16]) -> std::vec::Vec<LapBar> {
        power
            .iter()
            .map(|&watts| LapBar {
                power: (watts > 0).then_some(watts),
                speed_mm_s: Some(8_000),
            })
            .collect()
    }

    fn drawn(finished: &[LapBar], open: LapBar) -> std::vec::Vec<(Rect, u16)> {
        let mut canvas = Canvas::default();
        chart(&mut canvas, AT, AREA, finished, open);
        canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, color, .. } => Some((*rect, *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_best_lap_is_capped_and_the_open_lap_is_in_full_colour() {
        let finished = bars(&[200, 300, 250]);
        let shown = drawn(&finished, bars(&[150])[0]);
        let caps: std::vec::Vec<_> = shown.iter().filter(|(_, color)| *color == WHITE).collect();
        assert_eq!(caps.len(), 1);
        let best = shown[1].0;
        assert_eq!((best.top, best.bottom), (AREA.top, AREA.bottom));
        assert_eq!(caps[0].0.top, best.top);
        assert_eq!(shown.last().unwrap().1, POWER);
        let mut standing = shown.iter().filter(|(_, color)| *color != WHITE);
        assert!(standing.clone().count() == 4);
        assert!(standing.all(|(rect, _)| rect.bottom == AREA.bottom));
    }

    #[test]
    fn a_ride_without_power_charts_speed_and_a_lone_lap_has_no_cap() {
        let open = LapBar {
            power: None,
            speed_mm_s: Some(9_000),
        };
        let shown = drawn(&[], open);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].1, SPEED);
        let (left, width) = slot(AT, AREA, 0, 1);
        assert_eq!(width, AT.unit(MAX_BAR_UNITS));
        assert_eq!(left, (AREA.left + AREA.right - width) / 2);
    }

    #[test]
    fn every_kept_lap_fits_the_least_chart() {
        let form = Laps::forms((), AT)[0];
        let area = Rect {
            left: 0,
            top: 0,
            right: usize::from(form.width),
            bottom: usize::from(form.height),
        };
        for index in 0..BARS {
            let (left, width) = slot(AT, area, index, BARS);
            assert!(width >= AT.unit(LEAST_BAR_UNITS) && left + width <= area.right);
        }
    }
}
