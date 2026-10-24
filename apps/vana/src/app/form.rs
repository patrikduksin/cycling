//! Fitness, fatigue and form in the app: the FORM page under Settings,
//! PROFILE.
//!
//! The page folds the last seven weeks of saved rides with power into daily
//! stress (`ride::fitness`) from the ride index, at the FTP each ride's facts
//! kept. The index only reads once a list wants it, so opening FORM starts it,
//! and the page says READING RIDES until both journals are read. Nothing is
//! stored; the fold is redone whenever the rides, the FTP, the zone or the
//! minute change, into one buffer `Runtime` keeps.
use super::Runtime;
use super::history::WeeksKey;
use crate::ride::fitness::Daily;
use crate::screens::form::Mode;
use crate::screens::workout::Page;
use core::cell::{Cell, RefCell};
use device_api::input::Button;

#[derive(Default)]
pub(super) struct Kept {
    daily: RefCell<Daily>,
    shown: Cell<Option<(WeeksKey, Mode)>>,
}

impl Runtime {
    pub(super) fn open_form(&mut self, now: u64) {
        self.history.start_indexing();
        self.navigation.page = Page::Form;
        self.navigation.page_since = now;
    }

    /// BACK returns to Settings.
    pub(super) fn form_input(&mut self, button: Button, held: bool) {
        if button == Button::TopLeft && !held {
            self.navigation.page = Page::Settings;
        }
    }

    /// What the FORM page shows now.
    pub(super) fn form_mode(&self) -> Mode {
        let Some(utc_ms) = self.readings.live.utc_ms else {
            return Mode::NoClock;
        };
        if !self.counted() {
            return Mode::Reading;
        }
        let key = self.rides_key(utc_ms);
        if let Some((cached, mode)) = self.form.shown.get()
            && cached == key
        {
            return mode;
        }
        let zone = self.preferences.options.zone;
        let offset = |utc_s| zone.offset_s(utc_s);
        let mut daily = self.form.daily.borrow_mut();
        daily.reset(utc_ms, &offset);
        self.each_ride(|ride, stress| daily.add(ride.first_utc_ms, stress, &offset));
        let mode = if daily.powered == 0 {
            Mode::NoPower
        } else {
            Mode::Form(daily.form())
        };
        self.form.shown.set(Some((key, mode)));
        mode
    }
}
