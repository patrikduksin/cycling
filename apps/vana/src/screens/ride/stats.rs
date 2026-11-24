//! The ride's time in each zone, as the ride pages read it.
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
