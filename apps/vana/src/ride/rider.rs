//! The rider's body: weight, age and sex, kept for the two numbers that need
//! them. Heart-rate calories (Keytel et al., 2005) for the seconds a ride has
//! no power, and power per kilogram.
//!
//! Each field is unset until the rider enters it, and an unset field switches
//! its numbers off rather than guessing: without all three there are no
//! heart-rate calories, without weight no W/kg.
use core::ops::RangeInclusive;

pub const WEIGHT_KG: RangeInclusive<u8> = 30..=200;
pub const AGE_YEARS: RangeInclusive<u8> = 10..=99;
/// Where the first press of an unset weight or age lands.
pub const DEFAULT_WEIGHT_KG: u8 = 70;
pub const DEFAULT_AGE_YEARS: u8 = 30;

/// The stored form: weight, age and sex, each 0 when unset.
pub const LENGTH: usize = 3;

/// Keytel's two fits differ in every coefficient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sex {
    Male,
    Female,
}

/// Keytel's fit scaled by 10^4, in kJ per minute: intercept, per bpm, per kg
/// and per year of age.
struct Fit {
    intercept: i64,
    per_bpm: i64,
    per_kg: i64,
    per_year: i64,
}

const MALE: Fit = Fit {
    intercept: -550_969,
    per_bpm: 6_309,
    per_kg: 1_988,
    per_year: 2_017,
};
const FEMALE: Fit = Fit {
    intercept: -204_022,
    per_bpm: 4_472,
    per_kg: -1_263,
    per_year: 740,
};

/// 10^4 for the scaled fit, 60 s a minute and 4.184 kJ a kilocalorie.
const SCALED_KJ_PER_KCAL_SECOND: i128 = 2_510_400;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rider {
    weight_kg: u8,
    age_years: u8,
    sex: Option<Sex>,
}

/// The value after `step` from `value`, where 0 is unset. An unset value
/// starts at `default`. Stepping below the range clears it when `clear`, and
/// stops at the range's start otherwise, as a held button's repeats do.
fn stepped(value: u8, step: i32, range: &RangeInclusive<u8>, default: u8, clear: bool) -> u8 {
    if value == 0 {
        return if step == 0 { 0 } else { default };
    }
    let next = i32::from(value) + step;
    if next < i32::from(*range.start()) {
        if clear { 0 } else { *range.start() }
    } else {
        next.min(i32::from(*range.end())) as u8
    }
}

impl Rider {
    pub fn weight_kg(self) -> Option<u8> {
        (self.weight_kg != 0).then_some(self.weight_kg)
    }

    pub fn age_years(self) -> Option<u8> {
        (self.age_years != 0).then_some(self.age_years)
    }

    pub fn sex(self) -> Option<Sex> {
        self.sex
    }

    pub fn step_weight(self, step: i32, clear: bool) -> Self {
        Self {
            weight_kg: stepped(self.weight_kg, step, &WEIGHT_KG, DEFAULT_WEIGHT_KG, clear),
            ..self
        }
    }

    pub fn step_age(self, step: i32, clear: bool) -> Self {
        Self {
            age_years: stepped(self.age_years, step, &AGE_YEARS, DEFAULT_AGE_YEARS, clear),
            ..self
        }
    }

    /// Not set, then male, then female, then not set.
    pub fn next_sex(self) -> Self {
        Self {
            sex: match self.sex {
                None => Some(Sex::Male),
                Some(Sex::Male) => Some(Sex::Female),
                Some(Sex::Female) => None,
            },
            ..self
        }
    }

    pub fn encode(self) -> [u8; LENGTH] {
        let sex = match self.sex {
            None => 0,
            Some(Sex::Male) => 1,
            Some(Sex::Female) => 2,
        };
        [self.weight_kg, self.age_years, sex]
    }

    /// None for a value outside its range, so a damaged record is never read
    /// as some other body.
    pub fn decode([weight_kg, age_years, sex]: [u8; LENGTH]) -> Option<Self> {
        let sex = match sex {
            0 => None,
            1 => Some(Sex::Male),
            2 => Some(Sex::Female),
            _ => return None,
        };
        let in_range = |value: u8, range: &RangeInclusive<u8>| value == 0 || range.contains(&value);
        (in_range(weight_kg, &WEIGHT_KG) && in_range(age_years, &AGE_YEARS)).then_some(Self {
            weight_kg,
            age_years,
            sex,
        })
    }

    /// Watts per kilogram in tenths, once the weight is set.
    pub fn watts_per_kg_tenths(self, watts: u32) -> Option<u32> {
        let kg = u32::from(self.weight_kg()?);
        Some(watts.saturating_mul(10).saturating_add(kg / 2) / kg)
    }

    /// Kilocalories for `seconds` of heart rate readings that sum to
    /// `bpm_seconds`, from the Keytel fit. The fit is linear in heart rate,
    /// so the totals stand for every second. None until weight, age and sex
    /// are all set, and never negative.
    pub fn heart_rate_kcal(self, bpm_seconds: u64, seconds: u32) -> Option<u32> {
        let fit = match self.sex? {
            Sex::Male => MALE,
            Sex::Female => FEMALE,
        };
        let (kg, years) = (i64::from(self.weight_kg()?), i64::from(self.age_years()?));
        let resting = fit.intercept + fit.per_kg * kg + fit.per_year * years;
        let scaled = i128::from(seconds) * i128::from(resting)
            + i128::from(fit.per_bpm) * i128::from(bpm_seconds);
        let kcal = (scaled + SCALED_KJ_PER_KCAL_SECOND / 2).div_euclid(SCALED_KJ_PER_KCAL_SECOND);
        Some(kcal.clamp(0, i128::from(u32::MAX)) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rider(weight_kg: u8, age_years: u8, sex: Sex) -> Rider {
        Rider {
            weight_kg,
            age_years,
            sex: Some(sex),
        }
    }

    /// An hour at a steady heart rate, as `Totals` sums it.
    fn hour(rider: Rider, bpm: u64) -> Option<u32> {
        rider.heart_rate_kcal(bpm * 3600, 3600)
    }

    #[test]
    fn the_keytel_fit_matches_hand_computed_references() {
        // Male: (-55.0969 + 0.6309 x 140 + 0.1988 x 70 + 0.2017 x 30) kJ/min
        // is 53.1961 kJ/min, 3191.8 kJ an hour, 762.9 kcal.
        assert_eq!(hour(rider(70, 30, Sex::Male), 140), Some(763));
        // Female: (-20.4022 + 0.4472 x 140 - 0.1263 x 60 + 0.074 x 30)
        // is 36.8478 kJ/min, 528.4 kcal an hour.
        assert_eq!(hour(rider(60, 30, Sex::Female), 140), Some(528));
        // Ten minutes at 150 bpm, male 45 y 85 kg: 65.51 kJ/min, 156.6 kcal.
        assert_eq!(
            rider(85, 45, Sex::Male).heart_rate_kcal(150 * 600, 600),
            Some(157)
        );
    }

    #[test]
    fn calories_wait_for_weight_age_and_sex() {
        let full = rider(70, 30, Sex::Male);
        assert!(hour(full, 140).is_some());
        assert_eq!(hour(Rider { sex: None, ..full }, 140), None);
        assert_eq!(hour(full.step_weight(-1000, true), 140), None);
        assert_eq!(hour(full.step_age(-1000, true), 140), None);
        assert_eq!(hour(Rider::default(), 140), None);
    }

    #[test]
    fn a_resting_heart_rate_never_burns_a_negative_total() {
        let light = rider(30, 10, Sex::Female);
        assert_eq!(light.heart_rate_kcal(30 * 3600, 3600), Some(0));
        assert_eq!(light.heart_rate_kcal(0, 0), Some(0));
    }

    #[test]
    fn hostile_totals_saturate() {
        let rider = rider(200, 99, Sex::Male);
        assert_eq!(rider.heart_rate_kcal(u64::MAX, u32::MAX), Some(u32::MAX));
        assert_eq!(Rider::default().watts_per_kg_tenths(u32::MAX), None);
        assert!(
            Rider::default()
                .step_weight(1, true)
                .watts_per_kg_tenths(u32::MAX)
                .is_some()
        );
    }

    #[test]
    fn power_per_kilogram_rounds_to_a_tenth() {
        let rider = Rider::default().step_weight(1, true);
        assert_eq!(rider.weight_kg(), Some(70));
        assert_eq!(rider.watts_per_kg_tenths(215), Some(31));
        assert_eq!(rider.watts_per_kg_tenths(0), Some(0));
        assert_eq!(Rider::default().watts_per_kg_tenths(215), None);
    }

    #[test]
    fn stepping_starts_at_a_default_stays_in_range_and_clears_below_it() {
        let unset = Rider::default();
        assert_eq!(unset.step_weight(0, true), unset);
        assert_eq!(unset.step_weight(-1, true).weight_kg(), Some(70));
        assert_eq!(unset.step_age(1, true).age_years(), Some(30));
        let heavy = unset.step_weight(1, true).step_weight(1000, true);
        assert_eq!(heavy.weight_kg(), Some(200));
        assert_eq!(heavy.step_weight(1, true).weight_kg(), Some(200));
        let light = unset.step_weight(1, true).step_weight(-40, true);
        assert_eq!(light.weight_kg(), Some(30));
        assert_eq!(light.step_weight(-1, true).weight_kg(), None);
        let old = unset.step_age(1, true).step_age(1000, true);
        assert_eq!(old.age_years(), Some(99));
        assert_eq!(
            unset.step_age(1, true).step_age(-20, true).age_years(),
            Some(10)
        );
        assert_eq!(
            unset.step_age(1, true).step_age(-21, true).age_years(),
            None
        );
    }

    #[test]
    fn a_held_button_stops_at_the_bottom_of_the_range_and_only_a_tap_clears() {
        let low = Rider::default().step_weight(1, true).step_weight(-39, true);
        assert_eq!(low.weight_kg(), Some(31));
        let held = low.step_weight(-1, false).step_weight(-1, false);
        assert_eq!(held.weight_kg(), Some(30));
        assert_eq!(held.step_weight(-1, false).weight_kg(), Some(30));
        assert_eq!(held.step_weight(-1, true).weight_kg(), None);
        let young = Rider::default().step_age(1, true).step_age(-20, true);
        assert_eq!(young.age_years(), Some(10));
        assert_eq!(young.step_age(-1, false).age_years(), Some(10));
        assert_eq!(young.step_age(-1, true).age_years(), None);
    }

    #[test]
    fn sex_cycles_through_unset() {
        let unset = Rider::default();
        assert_eq!(unset.next_sex().sex(), Some(Sex::Male));
        assert_eq!(unset.next_sex().next_sex().sex(), Some(Sex::Female));
        assert_eq!(unset.next_sex().next_sex().next_sex(), unset);
    }

    #[test]
    fn stored_bodies_round_trip_and_damaged_ones_are_refused() {
        let chosen = rider(72, 41, Sex::Female);
        assert_eq!(chosen.encode(), [72, 41, 2]);
        assert_eq!(Rider::decode(chosen.encode()), Some(chosen));
        assert_eq!(Rider::decode([0, 0, 0]), Some(Rider::default()));
        for damaged in [
            [29, 30, 1],
            [201, 30, 1],
            [70, 9, 1],
            [70, 100, 1],
            [70, 30, 3],
        ] {
            assert_eq!(Rider::decode(damaged), None, "{damaged:?}");
        }
    }
}
