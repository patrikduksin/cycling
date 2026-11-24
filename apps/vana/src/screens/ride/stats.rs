//! The time-in-zone bar the summary draws.
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
    use crate::compose::data::Shares;
    use crate::ride::log::Sample;
    use crate::screens::workout::{Page, View};

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
}
