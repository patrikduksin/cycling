//! What tiles read: a frame's view, each number in its base unit, and the
//! gliding graphics alone for asking where a glide lands.
use super::ride::{ClimbRoad, GAUGE_WIDTH, Mark, Training, gauges};
use super::workout::{Graphics, View};
use crate::compose::data::{Count, Data, Effort, ListKind, Metric, OnGauge};
use crate::ride::zones::TrainingProfile;

impl Data for View<'_> {
    fn metric(&self, metric: Metric) -> Option<i32> {
        let totals = &self.totals;
        let sample = &self.sample;
        let int = |value: u64| i32::try_from(value).unwrap_or(i32::MAX);
        match metric {
            Metric::Cadence => sample
                .cadence_tenths
                .map(|tenths| i32::from((tenths + 5) / 10)),
            Metric::Distance => Some(int(totals.distance_mm() / 1000)),
            Metric::NormalizedPower => totals.normalized_power().map(i32::from),
            Metric::AvgPower => totals.avg_power().map(i32::from),
            Metric::AvgHeart => totals.avg_heart_bpm().map(i32::from),
            Metric::AvgSpeed => totals.avg_speed_mm_s().map(|mm_s| int(u64::from(mm_s))),
            Metric::Kcal => totals.kcal_for(self.rider).map(|kcal| int(u64::from(kcal))),
            Metric::Work => totals.work_kj().map(|kj| int(u64::from(kj))),
            Metric::Ascent => Some(int(u64::from((totals.ascent_dm() + 5) / 10))),
            Metric::Speed => sample.speed_mm_s.map(|mm_s| int(u64::from(mm_s))),
            Metric::Power => self.power_3s.map(i32::from),
            Metric::Heart => sample.heart_bpm.map(i32::from),
            Metric::Elapsed => Some(int(self.active_secs)),
            Metric::Grade => sample.gradient_tenths.map(i32::from),
        }
    }

    fn zone(&self, effort: Effort) -> Option<u8> {
        match effort {
            Effort::Power => self.power_3s.map(|watts| self.profile.power_zone(watts)),
            Effort::Heart => self
                .sample
                .heart_bpm
                .map(|bpm| self.profile.heart_zone(bpm)),
        }
    }

    fn list(&self, _: ListKind) -> u16 {
        0
    }

    fn road(&self) -> Option<ClimbRoad<'_>> {
        super::ride::climb::road(self)
    }

    fn training(&self) -> Option<&Training> {
        self.training.as_ref()
    }

    fn recording(&self) -> bool {
        self.state == crate::ride::log::Status::Recording
    }

    fn count(&self, count: Count) -> Option<i32> {
        use super::form::Mode as FormMode;
        let form = || match self.form() {
            FormMode::Form { form, left_out } => Some((form, left_out)),
            _ => None,
        };
        let mark = |index| super::bests::mark(self.bests(), index);
        let int = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
        match count {
            Count::Unsaved => Some(i32::from(self.unsaved)),
            Count::Goes => self.format.goes().map(i32::from),
            Count::BestsNote => self.bests_mode().note().map(i32::from),
            Count::Best(index) => Some(int(mark(index).value)),
            Count::BestDay(index) => Some(i32::from(mark(index).day)),
            Count::BestPerKg(index) => {
                let watts = mark(index).value;
                (index < 4 && watts != 0)
                    .then(|| self.rider.watts_per_kg_tenths(watts))
                    .flatten()
                    .map(int)
            }
            Count::FormNote => super::form::note(self.form()).map(i32::from),
            Count::Fitness => form().map(|(form, _)| i32::from(form.fitness)),
            Count::Fatigue => form().map(|(form, _)| i32::from(form.fatigue)),
            Count::Form => form().map(|(form, _)| i32::from(form.form)),
            Count::FormWeek(week) => form()
                .and_then(|(form, _)| form.trend.get(usize::from(week)).copied())
                .map(i32::from),
            Count::LeftOut => form().map(|(_, left_out)| i32::from(left_out)),
            Count::FtpNew => self.ftp_offer.map(|(new, _)| i32::from(new)),
            Count::FtpOld => self.ftp_offer.map(|(_, old)| i32::from(old)),
            Count::Seconds
            | Count::Farewell
            | Count::Charge
            | Count::ChargeFill
            | Count::Charging => None,
        }
    }

    fn map(&self) -> Option<&super::ride::MapFrame> {
        View::map(self)
    }

    fn summary(&self) -> Option<super::ride::summary::Facts<'_>> {
        use super::workout::Page;
        matches!(self.page, Page::Summary | Page::PastRide)
            .then(|| super::ride::summary::facts(self))
    }

    fn gauge(&self, effort: Effort) -> Option<OnGauge> {
        let [power, heart] = self.marks.gauges();
        on_gauge(match effort {
            Effort::Power => power,
            Effort::Heart => heart,
        })
    }

    fn target(&self, effort: Effort) -> Option<[OnGauge; 2]> {
        use crate::workout::model::Metric as Asked;
        let training = self.training.as_ref().filter(|training| !training.done)?;
        let band = training.now.band?;
        let asked = match band.metric {
            Asked::Power => Effort::Power,
            Asked::Heart => Effort::Heart,
            Asked::Cadence => return None,
        };
        (asked == effort).then(|| {
            [band.low, band.high].map(|value| {
                let profile = &self.profile;
                let (zone, (low, high)) = match effort {
                    Effort::Power => {
                        let zone = profile.power_zone(value);
                        (zone, profile.power_range(zone))
                    }
                    Effort::Heart => {
                        let zone = profile.heart_zone(value);
                        (zone, profile.heart_range(zone))
                    }
                };
                OnGauge {
                    zone,
                    within: u32::from(value).clamp(low, high) - low,
                    range: high - low,
                }
            })
        })
    }
}

fn on_gauge(mark: Option<Mark>) -> Option<OnGauge> {
    mark.map(|mark| OnGauge {
        zone: mark.zone,
        within: u32::from(mark.fill),
        range: GAUGE_WIDTH as u32,
    })
}

/// The gliding graphics alone: where a glide lands, without a frame's view.
pub struct Gliding<'a> {
    pub graphics: &'a Graphics,
    pub profile: &'a TrainingProfile,
}

impl Data for Gliding<'_> {
    fn metric(&self, _: Metric) -> Option<i32> {
        None
    }

    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }

    fn list(&self, _: ListKind) -> u16 {
        0
    }

    fn gauge(&self, effort: Effort) -> Option<OnGauge> {
        let [power, heart] = gauges(self.graphics, self.profile);
        on_gauge(match effort {
            Effort::Power => power,
            Effort::Heart => heart,
        })
    }
}
