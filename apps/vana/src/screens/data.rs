//! What tiles read: a frame's view, each number in its base unit, and the
//! gliding graphics alone for asking where a glide lands.
use super::ride::{ClimbRoad, GAUGE_WIDTH, Mark, Training, gauges};
use super::workout::{Graphics, View};
use crate::compose::data::{Count, Data, Effort, Metric, OnGauge, PageData, Shares};
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
            Metric::Moving => Some(int(totals.moving_ms() / 1000)),
            Metric::MaxSpeed => totals.max_speed_mm_s().map(|mm_s| int(u64::from(mm_s))),
            Metric::Intensity => totals
                .intensity_hundredths(self.profile.ftp_watts)
                .map(i32::from),
            Metric::Stress => totals
                .stress_score(self.profile.ftp_watts)
                .map(|score| int(u64::from(score))),
            Metric::Paused => Some(int(u64::from(self.paused_secs))),
            Metric::Altitude => totals.altitude_dm().map(|dm| (dm + 5).div_euclid(10)),
            Metric::ClimbDistance | Metric::ClimbGain | Metric::ToTop | Metric::UpToGo => {
                super::ride::climb::reading(self, metric)
            }
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

    fn sensors(&self) -> Option<&crate::screens::sensors::Menu> {
        self.sensor_pages()
    }

    fn done(&self) -> Option<&crate::screens::done::Done> {
        self.done()
    }

    fn phone(&self) -> Option<crate::screens::phone::Shown<'_>> {
        self.phone()
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

    fn theme(&self) -> &'static super::theme::Theme {
        if self.light {
            &super::theme::LIGHT
        } else {
            &super::theme::DARK
        }
    }

    fn status(&self) -> crate::compose::data::Status {
        crate::compose::data::Status {
            gps: self.gps,
            battery: self.battery,
        }
    }

    fn clock(&self) -> &str {
        self.time.as_str()
    }

    fn page(&self) -> PageData<'_> {
        use super::workout::Page;
        match self.page {
            Page::Boot => PageData::Brand(self.reveal),
            Page::Home => self.home().map_or(PageData::None, PageData::Home),
            _ => PageData::None,
        }
    }

    fn count(&self, count: Count) -> Option<i32> {
        match count {
            Count::Unsaved => Some(i32::from(self.unsaved)),
            Count::Goes => self.format.goes().map(i32::from),
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

    fn zone_time(&self, effort: Effort) -> Option<Shares> {
        let time = &self.zone_time;
        match effort {
            Effort::Power => shares(&time.power_ms),
            Effort::Heart => shares(&time.heart_ms),
        }
    }

    fn auto_paused(&self) -> bool {
        self.auto_paused
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

fn shares<const N: usize>(time: &[u64; N]) -> Option<Shares> {
    let total: u64 = time.iter().sum();
    if total == 0 {
        return None;
    }
    let mut ends = [Shares::WHOLE; 7];
    let mut so_far = 0;
    for (end, ms) in ends.iter_mut().zip(time) {
        so_far += ms;
        *end = (so_far * u64::from(Shares::WHOLE) / total) as u16;
    }
    Some(Shares(ends))
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

    fn gauge(&self, effort: Effort) -> Option<OnGauge> {
        let [power, heart] = gauges(self.graphics, self.profile);
        on_gauge(match effort {
            Effort::Power => power,
            Effort::Heart => heart,
        })
    }
}
