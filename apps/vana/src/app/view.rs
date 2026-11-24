//! VANA's frame: four tiers that write `Runtime`, around one read-only draw.
//! `Runtime::frame` calls three tiers, the draw, then the fourth tier:
//!
//! 1. `announce_ride` says what the ride's status became, even while the
//!    screen is blank.
//! 2. `unless_blank` stops on a blank or turning-off screen. It then latches the
//!    numbers and takes the motion. It stops again unless a frame is due.
//! 3. `when_due` sets the base pace, then makes every other write the frame
//!    makes: the power sheet's animation, the page moves
//!    and toasts, the map, the sensor menu and the shell's activity. Under a
//!    power sheet it stops after the sheet's animation. It also reads the
//!    position and the radar channel for the view, at their place in that
//!    order, so the trace stream stays the same.
//! 4. `present(&self)`, in `presentation.rs`, fills the `View` and draws it. It
//!    writes nothing in `Runtime`. It returns `Presented`, a small `Copy` value:
//!    a drawn power sheet, or a drawn page with the draw result, the page and
//!    the gliding graphics' pixels.
//! 5. `after_view` applies `Presented`. For a page it calls `motion.drew`
//!    first, then sets the timing milestones and FORMAT's `Drawn`, then the
//!    next frame time. The next frame time reads the motion after `drew`, so
//!    `after_view` computes it, not `present`.
//!
//! A frame's `View` lives only in `present`'s stack frame, and `screens::workout`
//! asserts its size on the device. The tiers and `present` are
//! `#[inline(never)]`, so no tier's frame sits under another's. A new write in
//! the frame goes into the tier that runs at its moment, never into `present`.
use super::Runtime;
use super::navigation::To;
use super::presentation::{
    ANIMATED_FRAME_MS, BATTERY_TOAST_MS, BRAND_MS, Display, MOTION_FRAME_MS, STATIC_FRAME_MS,
    arrived_sample, radar_reports,
};
use crate::compose::builtin::ScreenId;
use crate::compose::registry::MAP;
use crate::ride::log::{Sample, Status};
use crate::screens::chrome::{Toast, Tone};
use crate::screens::power::Sheet as PowerSheet;
use crate::screens::ride::{Pixels, RidePage, map};
use crate::screens::workout::{Page, Text};
use crate::sensors::ant::device_type;
use core::fmt::Write;
use device_api::ant::Ant;
use device_api::display::Display as Panel;
use device_api::input::InputSource;
use device_api::positioning::{Positioning, Snapshot as Position};
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_services::why;
use firmware_shell::shell::{Drawn, Screen, Shell};

#[derive(Clone, Copy)]
struct Frame {
    shown: Sample,
}

#[derive(Clone, Copy)]
struct Due {
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

impl Runtime {
    /// Draw a frame when one is due. Call once per pass.
    pub fn frame<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        now: u64,
        ant: &impl Ant,
        position: &impl Positioning,
    ) {
        self.announce_ride(now);
        let Some(frame) = self.unless_blank(system, now) else {
            return;
        };
        let due = self.when_due(system, now, ant, position);
        match due.shows {
            Shows::Page { radar_paired, .. } => {
                self.compose(radar_paired, now);
                self.refresh_map(radar_paired, now);
                self.refresh_done();
            }
            Shows::Sheet(sheet) => self.composition.show(
                sheet.screen(),
                self.device,
                super::overrides::shown(&self.overrides),
            ),
        }
        let presented = self.present(system, due.shows, now);
        self.after_view(presented, frame, now);
    }

    /// Fill the map again where the map page's tree placed it, while it
    /// shows.
    fn refresh_map(&mut self, radar: bool, now: u64) {
        let Some(shown) = self
            .composition
            .laids()
            .get(ScreenId::RideMap)
            .and_then(|laid| laid.box_of(MAP))
        else {
            return;
        };
        let light = self.light();
        super::course::refresh_map(
            &mut self.track,
            now,
            self.readings.live.speed_mm_s,
            radar,
            map::ground(&self.device.scaled, shown),
            light,
            self.micros_clock,
        );
    }

    fn announce_ride(&mut self, now: u64) {
        if self.power.view().is_some() || self.navigation.page() == Page::Boot {
            return;
        }
        let pages_over_pause =
            self.ride.auto().paused() && super::course::course_placed(&self.track);
        let unsynced = self.shown_ride_unsynced();
        self.navigation
            .announce(self.ride.active().status(), pages_over_pause, unsynced, now);
    }

    #[inline(never)]
    fn unless_blank<D: Panel, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &Shell<D, I, P, B>,
        now: u64,
    ) -> Option<Frame> {
        // Once the device accepted turning off, the last frame stays.
        if system.foreground == Screen::Blank || self.turning_off() {
            why!(
                "frame.blank",
                blank = system.foreground == Screen::Blank,
                turning_off = self.turning_off()
            );
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
            why!("frame.suspended");
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
                why!("frame.reveal_skipped", home_at, brand_ms = BRAND_MS);
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
        let animated = matches!(page, Page::Ride | Page::EndRide)
            || sheet.is_some_and(PowerSheet::animated)
            || self.readings.battery.charging();
        let pace_ms = if motion {
            MOTION_FRAME_MS
        } else if animated {
            ANIMATED_FRAME_MS
        } else {
            STATIC_FRAME_MS
        };
        why!(
            "frame.pace",
            pace_ms,
            motion,
            animated,
            page,
            sheet = sheet.is_some()
        );
        self.display = Display::Active {
            next_frame_ms: now.saturating_add(pace_ms),
        };
        self.power.animate(now);
        if let Some(sheet) = self.power.view() {
            return Due {
                shows: Shows::Sheet(sheet),
            };
        }
        let first_page = self.first_page();
        let navigation = &mut self.navigation;
        let first_frame = navigation.first_frame_at(now);
        if navigation.page() == Page::Boot && now.saturating_sub(first_frame) >= BRAND_MS {
            why!("frame.brand_over", first_frame, first_page);
            navigation.go(To::Page(first_page));
        }
        // A ride opens on its first page: the workout page during a workout.
        let first = self.first_ride_page();
        self.navigation.set_first_page(first);
        self.announce_ride(now);
        let status = self.ride.active().status();
        let navigation = &mut self.navigation;
        // The ride is over: tell the rider once what the calibration set, over
        // the ride's own saved toast.
        if status != Status::Saved && !self.ride.open() {
            self.readings.wheel_news = None;
        }
        if status == Status::Saved
            && navigation.page() != Page::Boot
            && let Some(learned) = self.readings.wheel_news.take()
        {
            why!("frame.wheel_set", learned);
            let mut text = Text::<20>::new();
            let _ = write!(text, "WHEEL SET {learned} MM");
            self.navigation
                .show(Toast::new(text.as_str(), Tone::Good), BATTERY_TOAST_MS, now);
        }
        // A page the rider turned off, or the workout page once its workout
        // is gone, gives way to the main page.
        if !self.ride_pages().contains(self.navigation.ride_page()) {
            why!(
                "frame.ride_page_off",
                ride_page = self.navigation.ride_page()
            );
            self.navigation.go(To::RidePage(RidePage::Main));
        }
        let page = self.navigation.page();
        let gps = position.snapshot(now);
        let radar_paired = ant.channel(device_type::RADAR, now).is_some();
        self.navigation.expire_toast(now);
        match page {
            Page::Home | Page::Sensors => self.refresh_sensor_menu(ant, now),
            // Keep the screen lit while riding; a finished ride may dim and idle off.
            Page::Ride if self.recorder_busy() => system.activity(now),
            _ => {}
        }
        Due {
            shows: Shows::Page { gps, radar_paired },
        }
    }

    #[inline(never)]
    fn after_view(&mut self, presented: Presented, frame: Frame, now: u64) {
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
                    if page == Page::Home {
                        self.timing.first_ready(drawn);
                    }
                    if page == Page::Format {
                        self.format_event(super::format::Event::Drawn(drawn));
                    }
                    self.timing.frame(drawn);
                }
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
