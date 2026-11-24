//! What tiles read: a frame's view, each number in its base unit, and the
//! gliding graphics alone for asking where a glide lands.
use super::chrome::SECONDARY;
use super::ride::{ClimbRoad, GAUGE_WIDTH, LapBar, Mark, Training, gauges};
use super::settings::{NOTES, STATUS, VALUES};
use super::workout::Page;
use super::workout::{Graphics, View};
use crate::compose::data::{
    Count, Data, Edited, Effort, Entry, Glance, ListKind, Listed, Metric, OnGauge, PageData, Shares,
};
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
            Metric::LapTime
            | Metric::LapDistance
            | Metric::LapPower
            | Metric::LapSpeed
            | Metric::LapHeart
            | Metric::LapNumber
            | Metric::PrevLapNumber
            | Metric::PrevLapTime
            | Metric::PrevLapPower
            | Metric::SpeedDiff
            | Metric::PowerDiff => self.lap_metric(metric),
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

    fn list(&self, list: ListKind) -> u16 {
        match list {
            ListKind::Workouts => self.picker().count() as u16,
            ListKind::Rides => self.rides().count as u16,
            ListKind::Settings => self.sections.rows(self.cursor).0.len() as u16,
            ListKind::Sensors | ListKind::Nearby => {
                self.sensor_pages().map_or(0, |menu| menu.count(list))
            }
        }
    }

    fn listed(&self, list: ListKind) -> Listed {
        match list {
            ListKind::Workouts => {
                let picker = self.picker();
                Listed {
                    offset: self.scroll,
                    focus: Some(picker.cursor as u16),
                    pressed: picker.pressed.map(|index| index as u16),
                }
            }
            ListKind::Rides => {
                let rides = self.rides();
                Listed {
                    offset: self.scroll,
                    focus: Some(rides.cursor as u16),
                    pressed: rides.pressed.map(|index| index as u16),
                }
            }
            ListKind::Settings => {
                let (sections, cursor) = (self.sections, self.cursor);
                let first = sections.rows(cursor).1;
                Listed {
                    offset: self.scroll,
                    focus: Some((cursor % sections.fields() - first) as u16),
                    pressed: self.pressed_setting(),
                }
            }
            ListKind::Sensors | ListKind::Nearby => Listed {
                offset: self.scroll,
                focus: self.sensor_pages().and_then(|menu| menu.focus(list)),
                pressed: None,
            },
        }
    }

    fn heading(&self, list: ListKind) -> &str {
        match list {
            ListKind::Settings => {
                let sections = self.sections;
                sections.title(sections.section(self.cursor).0)
            }
            ListKind::Rides | ListKind::Workouts | ListKind::Sensors | ListKind::Nearby => {
                list.name()
            }
        }
    }

    fn saving(&self) -> Option<(&str, bool)> {
        if self.page != Page::Settings || self.toast.is_some() {
            return None;
        }
        let status = self.lines()[STATUS].as_str();
        Some((status, matches!(status, "SAVED" | "SAVING")))
    }

    fn edited(&self) -> Option<Edited<'_>> {
        let row = self.sections.row(self.cursor);
        let lines = self.lines();
        (self.page == Page::Settings && self.editing).then(|| Edited {
            label: row.label(),
            value: lines[VALUES].as_str(),
            note: lines[NOTES].as_str(),
            step: row.step(),
        })
    }

    fn sensors(&self) -> Option<&crate::screens::sensors::Menu> {
        self.sensor_pages()
    }

    fn glance(&self) -> Glance {
        self.rides().glance
    }

    fn entry(&self, list: ListKind, index: u16) -> Option<Entry<'_>> {
        match list {
            ListKind::Workouts => self.picker().entry(usize::from(index)),
            ListKind::Rides => self.rides().entry(usize::from(index)),
            ListKind::Settings => {
                let rows = self.sections.rows(self.cursor).0;
                let slot = usize::from(index);
                let lines = self.lines();
                Some(Entry {
                    title: lines[VALUES + slot].as_str(),
                    detail: rows.get(slot)?.label(),
                    note: lines[NOTES + slot].as_str(),
                    note_color: SECONDARY,
                    note_first: false,
                    chosen: false,
                })
            }
            ListKind::Sensors | ListKind::Nearby => None,
        }
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

    fn page(&self) -> PageData<'_> {
        use super::workout::Page;
        match self.page {
            Page::Boot => PageData::Brand(self.reveal),
            Page::Home => self.home().map_or(PageData::None, PageData::Home),
            Page::Ready => PageData::Ready(self.readiness()),
            Page::Setup => PageData::Setup {
                step: self.setup_step(),
                notes: self.lines(),
            },
            Page::Phone => PageData::Phone {
                lines: self.lines(),
                qr: self.qr(),
                light: self.light,
            },
            _ => PageData::None,
        }
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

    fn zone_time(&self, effort: Effort) -> Option<Shares> {
        let time = &self.zone_time;
        match effort {
            Effort::Power => shares(&time.power_ms),
            Effort::Heart => shares(&time.heart_ms),
        }
    }

    fn laps(&self) -> Option<(&[LapBar], LapBar)> {
        Some((self.extra.lap_bars()?.as_slice(), self.extra.lap()?.into()))
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

impl View<'_> {
    /// The open lap's readings and the last finished lap's, while the lap
    /// page shows them.
    fn lap_metric(&self, metric: Metric) -> Option<i32> {
        let lap = self.extra.lap()?;
        let previous = self.extra.previous_lap()?;
        let int = |value: u64| i32::try_from(value).unwrap_or(i32::MAX);
        let tenths = |mm_s: u32| i32::try_from(u64::from(mm_s) * 36 / 1000).unwrap_or(i32::MAX);
        match metric {
            Metric::LapTime => Some(int(lap.time_ms / 1000)),
            Metric::LapDistance => Some(int(lap.distance_mm / 1000)),
            Metric::LapPower => lap.avg_power().map(i32::from),
            Metric::LapSpeed => lap.avg_speed_mm_s().map(|mm_s| int(u64::from(mm_s))),
            Metric::LapHeart => lap.avg_heart_bpm().map(i32::from),
            Metric::LapNumber => Some(i32::from(lap.number)),
            Metric::PrevLapNumber => previous.map(|previous| i32::from(previous.number)),
            Metric::PrevLapTime => previous.map(|previous| int(previous.time_ms / 1000)),
            Metric::PrevLapPower => previous?.avg_power().map(i32::from),
            Metric::SpeedDiff => {
                Some(tenths(lap.avg_speed_mm_s()?) - tenths(previous?.avg_speed_mm_s()?))
            }
            Metric::PowerDiff => {
                Some(i32::from(lap.avg_power()?) - i32::from(previous?.avg_power()?))
            }
            _ => None,
        }
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
