//! The readings the rider chose for the main page's lower line.
//!
//! The line has two slots, left and right. Each holds one of
//! [`Metric::LOWER`], by the id the preference record stores (see
//! `app::preferences`). The defaults are the cadence and the distance the
//! page always showed.
use crate::compose::data::Metric;

/// Bytes the slots take in a record: one id per slot.
pub const LENGTH: usize = 2;

/// Which reading each slot of the lower line shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slots {
    pub left: Metric,
    pub right: Metric,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            left: Metric::Cadence,
            right: Metric::Distance,
        }
    }
}

impl Slots {
    pub const fn encode(self) -> [u8; LENGTH] {
        [self.left.id(), self.right.id()]
    }

    /// The slots a record's bytes stand for; none when an id means nothing.
    pub const fn decode([left, right]: [u8; LENGTH]) -> Option<Self> {
        match (lower(left), lower(right)) {
            (Some(left), Some(right)) => Some(Self { left, right }),
            _ => None,
        }
    }
}

const fn lower(id: u8) -> Option<Metric> {
    if (id as usize) < Metric::LOWER.len() {
        Some(Metric::LOWER[id as usize])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Text;

    #[test]
    fn lower_ids_are_the_position_in_lower_and_round_trip() {
        for (at, metric) in Metric::LOWER.into_iter().enumerate() {
            assert_eq!(usize::from(metric.id()), at);
            assert_eq!(lower(metric.id()), Some(metric));
        }
        for id in Metric::LOWER.len() as u8..=u8::MAX {
            assert_eq!(lower(id), None);
        }
    }

    #[test]
    fn next_visits_every_lower_reading_once_and_wraps() {
        let mut seen = std::vec::Vec::new();
        let mut metric = Metric::Cadence;
        for _ in 0..Metric::LOWER.len() {
            seen.push(metric);
            metric = metric.next();
        }
        assert_eq!(metric, Metric::Cadence);
        assert_eq!(seen, Metric::LOWER);
    }

    #[test]
    fn a_count_past_four_digits_reads_in_thousands() {
        let text = |count: Option<i32>| {
            let mut value = Text::<12>::new();
            Metric::Kcal.write(count, &mut value);
            value
        };
        assert_eq!(text(None).as_str(), "--");
        assert_eq!(text(Some(9_999)).as_str(), "9999");
        assert_eq!(text(Some(10_000)).as_str(), "10K");
        assert_eq!(text(Some(10_499)).as_str(), "10K");
        assert_eq!(text(Some(10_500)).as_str(), "11K");
        assert_eq!(text(Some(i32::MAX)).as_str(), "999K");
    }

    #[test]
    fn the_defaults_are_the_cadence_and_distance_the_page_always_had() {
        assert_eq!(Slots::default().encode(), [0, 1]);
        assert_eq!(Slots::decode([0, 1]), Some(Slots::default()));
        assert_eq!(Slots::decode([9, 1]), None);
        assert_eq!(Slots::decode([0, 255]), None);
    }
}
