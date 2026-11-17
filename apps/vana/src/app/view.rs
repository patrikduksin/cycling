use super::Runtime;
use super::navigation::To;
use super::presentation::{
    ANIMATED_FRAME_MS, BATTERY_TOAST_MS, BRAND_MS, Display, MOTION_FRAME_MS, SCROLL_FRAME_MS,
    STATIC_FRAME_MS, arrived_sample, radar_reports,
};
use crate::ride::elevation::Trace;
use crate::ride::log::{Sample, Status};
use crate::screens::chrome::{Rect, Toast, Tone};
use crate::screens::power::Sheet as PowerSheet;
use crate::screens::ride::summary::SummaryPage;
use crate::screens::ride::{ElevationChart, Pixels, RidePage};
use crate::screens::workout::{Page, Text};
use crate::sensors::ant::device_type;
use core::fmt::Write;
use device_api::ant::Ant;
use device_api::display::Display as Panel;
use device_api::input::InputSource;
use device_api::positioning::{Positioning, Snapshot as Position};
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_shell::shell::{Drawn, Screen, Shell};

#[derive(Clone, Copy)]
struct Frame {
    shown: Sample,
}

#[derive(Clone, Copy)]
struct Due {
    moving: bool,
    shows: Shows,
}

#[derive(Clone, Copy)]
pub(super) enum Shows {
    Sheet(PowerSheet),
    Page {
        gps: Option<Position>,
        radar_paired: bool,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Presented {
    Sheet {
        ok: bool,
    },
    Page {
        page: Page,
        drawn: Drawn,
        pixels: Pixels,
    },
}

#[derive(Default)]
pub(super) struct Chart {
    chart: ElevationChart,
    charted: Option<u32>,
}

impl Chart {
    pub(super) fn chart(&self) -> &ElevationChart {
        &self.chart
    }

    fn refresh(&mut self, trace: &Trace, rect: Rect) {
        if self.charted != Some(trace.generation()) {
            self.charted = Some(trace.generation());
            self.chart.refresh(trace, rect);
        }
    }
}

impl Runtime {
    /// Draw a frame when one is due. Call once per pass.
    pub fn frame<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        ant: &impl Ant,
        position: &impl Positioning,
    ) {
        self.every_pass(system, now, position);
        let Some(frame) = self.unless_blank(system, now) else {
            return;
        };
        let due = self.when_due(system, now, ant, position);
        let presented = self.present(system, due.shows, now);
        self.after_view(presented, frame, due.moving, now);
    }

    #[inline(never)]
    fn every_pass<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        position: &impl Positioning,
    ) {
        self.follow_daylight(now, position);
        system.set_curve(
            self.preferences
                .options
                .auto_brightness
                .then_some(self.policy().light_curve),
        );
    }

    #[inline(never)]
    fn unless_blank<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &Shell<D, I, P, B>,
        now: u64,
    ) -> Option<Frame> {
        // Once the device accepted turning off, the last frame stays.
        if system.foreground == Screen::Blank || self.turning_off() {
            return None;
        }
        self.latch_numbers(now);
        let shown = self.shown.sample();
        self.motion.take(
            &arrived_sample(&self.readings),
            &radar_reports(&self.readings, now),
            self.policy().glides,
            now,
        );
        // A new number, or a glide that moves a graphic, draws rather than
        // waiting for the page's usual pace.
        let due = self.motion_due(&shown, now);
        let Display::Active { next_frame_ms } = &mut self.display else {
            return None;
        };
        if let Some(due) = due {
            *next_frame_ms = (*next_frame_ms).min(due);
        }
        if now < *next_frame_ms {
            return None;
        }
        // A reveal frame that could still be drawing when the brand time
        // ends would make Home late; skip it and draw Home on time.
        if self.navigation.page() == Page::Boot
            && self.power.view().is_none()
            && let Some(first) = self.navigation.first_frame()
        {
            let home_at = first + BRAND_MS;
            if now < home_at && now + MOTION_FRAME_MS > home_at {
                *next_frame_ms = home_at;
                return None;
            }
        }
        Some(Frame { shown })
    }

    #[inline(never)]
    fn when_due<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        ant: &impl Ant,
        position: &impl Positioning,
    ) -> Due {
        let sheet = self.power.view();
        let page = self.navigation.page();
        let motion = matches!(sheet, Some(PowerSheet::TurningOff { .. }))
            || (sheet.is_none() && page == Page::Boot);
        let animated = matches!(
            page,
            Page::Ride | Page::Ready | Page::EndRide | Page::DiscardRide | Page::Summary
        ) || sheet.is_some_and(PowerSheet::animated)
            || self.readings.battery.charging();
        self.display = Display::Active {
            next_frame_ms: now.saturating_add(if motion {
                MOTION_FRAME_MS
            } else if animated {
                ANIMATED_FRAME_MS
            } else {
                STATIC_FRAME_MS
            }),
        };
        // A card under a resting finger lights up once the delay passes.
        self.navigation.touches_mut().tick(now);
        let moving = self.list_frame(now);
        self.refresh_chart();
        self.power.animate(now);
        if let Some(sheet) = self.power.view() {
            return Due {
                moving,
                shows: Shows::Sheet(sheet),
            };
        }
        let first_page = self.first_page();
        let navigation = &mut self.navigation;
        navigation.lapse_returns();
        let first_frame = navigation.first_frame_at(now);
        if navigation.page() == Page::Boot && now.saturating_sub(first_frame) >= BRAND_MS {
            navigation.go(To::Page(first_page));
        }
        // A ride opens on its first page: the workout page during a workout.
        let first = self.first_ride_page();
        let navigation = &mut self.navigation;
        navigation.set_first_page(first);
        let status = self.ride.active().status();
        let pages_over_pause =
            self.ride.auto().paused() && super::course::course_placed(&self.track);
        if navigation.page() != Page::Boot {
            navigation.announce(status, pages_over_pause, now);
        }
        // The ride is over: tell the rider once what the calibration set, over
        // the ride's own saved toast.
        if status != Status::Saved && !self.ride.open() {
            self.readings.wheel_news = None;
        }
        if status == Status::Saved
            && navigation.page() != Page::Boot
            && let Some(learned) = self.readings.wheel_news.take()
        {
            let mut text = Text::<20>::new();
            let _ = write!(text, "WHEEL SET {learned} MM");
            self.navigation
                .show(Toast::new(text.as_str(), Tone::Good), BATTERY_TOAST_MS, now);
        }
        // Over the ride's own news: its workout could not be read.
        if core::mem::take(&mut self.training.missing) {
            self.navigation.toast("WORKOUT MISSING", Tone::Warn, now);
        }
        if status == Status::Recording {
            self.history_ride_started();
        }
        // A page the rider turned off, or the workout page once its workout
        // is gone, gives way to the main page.
        if !self.ride_pages().contains(self.navigation.ride_page()) {
            self.navigation.go(To::RidePage(RidePage::Main));
        }
        let page = self.navigation.page();
        let gps = position.snapshot(now);
        let radar_paired = ant.channel(device_type::RADAR, now).is_some();
        if page == Page::Ride && self.navigation.ride_page() == RidePage::Map {
            let light = self.light();
            super::course::refresh_map(
                &mut self.track,
                now,
                self.readings.live.speed_mm_s,
                radar_paired,
                &self.layout.map,
                light,
                self.micros_clock,
            );
        }
        self.navigation.expire_toast(now);
        match page {
            Page::Ready | Page::Home | Page::Sensors => self.refresh_sensor_menu(ant, now),
            Page::Phone => {
                // The codes stay readable while a phone connects.
                system.activity(now);
                self.phone_joined();
            }
            // Keep the screen lit while riding; a finished ride may dim and idle off.
            Page::Ride if self.recording() => system.activity(now),
            _ => {}
        }
        Due {
            moving,
            shows: Shows::Page { gps, radar_paired },
        }
    }

    /// Chart the ride's trace again when it changed, as the summary page that
    /// shows it opens.
    fn refresh_chart(&mut self) {
        let showing = matches!(self.navigation.page(), Page::Summary | Page::PastRide)
            && SummaryPage::at(self.navigation.summary_page(), self.has_elevation())
                == SummaryPage::Elevation;
        if showing {
            let rect = self.layout.summary.elevation.chart;
            self.chart.refresh(self.ride.trace(), rect);
        }
    }

    #[inline(never)]
    fn after_view(&mut self, presented: Presented, frame: Frame, moving: bool, now: u64) {
        match presented {
            Presented::Sheet { ok } => {
                self.power_drawn(ok);
                if ok {
                    self.timing.frame(self.drawn_at(now));
                }
            }
            Presented::Page {
                page,
                drawn,
                pixels,
            } => {
                self.motion.drew(&frame.shown, pixels, now);
                if reached_panel(drawn) {
                    let drawn = self.drawn_at(now);
                    self.timing.first_frame(drawn);
                    if matches!(page, Page::Home | Page::Setup) {
                        self.timing.first_ready(drawn);
                    }
                    if page == Page::Format {
                        self.format_event(super::format::Event::Drawn(drawn));
                    }
                    self.timing.frame(drawn);
                }
            }
        }
        if let Display::Active { next_frame_ms } = &mut self.display {
            if moving {
                *next_frame_ms = (*next_frame_ms).min(now + SCROLL_FRAME_MS);
            }
            if let Some(due) = self.navigation.touches().due() {
                *next_frame_ms = (*next_frame_ms).min(due);
            }
        }
        let due = self.motion_due(&frame.shown, now);
        if self.navigation.page() == Page::Ride
            && let Display::Active { next_frame_ms } = &mut self.display
        {
            if let Some(due) = due {
                *next_frame_ms = (*next_frame_ms).min(due);
            }
            // Motion frames shift the frame phase, so the ride clock asks for
            // a frame when its next second turns rather than up to a frame late.
            let active_ms = self.ride.active_ms(now);
            if self.ride.active_ms(now + 1000) > active_ms {
                *next_frame_ms = (*next_frame_ms).min(now + 1000 - active_ms % 1000);
            }
        }
        // Open Home exactly when the brand flash ends, not on the next animation frame.
        if self.navigation.page() == Page::Boot
            && let (Some(first), Display::Active { next_frame_ms }) =
                (self.navigation.first_frame(), &mut self.display)
        {
            *next_frame_ms = (*next_frame_ms).min(first + BRAND_MS);
        }
    }
}

fn reached_panel(drawn: Drawn) -> bool {
    drawn == Drawn::Sent
}
