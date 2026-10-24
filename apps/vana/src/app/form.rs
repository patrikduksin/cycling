//! Fitness, fatigue and form in the app: the FORM page under Settings,
//! PROFILE.
//!
//! The page folds the last 120 days of saved rides with power into daily
//! stress (`ride::fitness`) from the ride index, at the FTP each ride's facts
//! kept. The index only reads once a list wants it, so opening FORM starts it,
//! and the page says READING RIDES until both journals are read. Nothing is
//! stored. The fold runs in the service tick while FORM or Settings is open
//! (the PROFILE row shows the result), whenever the
//! rides, the FTP, the zone or the minute change, into the one buffer
//! `Runtime` keeps; drawing only copies the answer, so the render path's stack
//! does not carry the fold.
use super::Runtime;
use super::history::WeeksKey;
use crate::ride::fitness::Daily;
use crate::screens::form::{Mode, signed, verdict};
use crate::screens::workout::{Page, Text};
use core::cell::RefCell;
use core::fmt::Write;
use device_api::input::Button;

#[derive(Default)]
pub(super) struct Kept {
    /// Behind a `RefCell` so the fold can fill it in place while it reads
    /// the rides through `&self`, with no 256 byte copy on the tick's stack.
    daily: RefCell<Daily>,
    /// What the rides read as of the last fold, and when.
    folded: Option<(WeeksKey, Mode)>,
    /// What the page shows.
    pub(super) mode: Mode,
}

impl Kept {
    /// The FORM row's value in Settings: the latest fold.
    pub(super) fn summary(&self) -> Text<38> {
        let mut text = Text::new();
        let _ = match self.folded {
            Some((_, Mode::Form { form, .. })) => {
                let mut number = Text::<12>::new();
                signed(&mut number, form.form);
                write!(text, "{} {}", number.as_str(), verdict(form.form).0)
            }
            _ => text.push_str("OPEN TO SEE"),
        };
        text
    }
}

impl Runtime {
    pub(super) fn open_form(&mut self, now: u64) {
        self.history.start_indexing();
        self.form.mode = Mode::Reading;
        self.navigation.page = Page::Form;
        self.navigation.page_since = now;
    }

    /// BACK returns to Settings.
    pub(super) fn form_input(&mut self, button: Button, held: bool) {
        if button == Button::TopLeft && !held {
            self.navigation.page = Page::Settings;
        }
    }

    /// Keep what the FORM page shows up to date while it is open, and the
    /// PROFILE row's value while Settings is. The minute-keyed cache bounds the
    /// work to one fold a minute, or when the rides or the FTP change.
    pub(super) fn service_form(&mut self) {
        if !matches!(self.navigation.page, Page::Form | Page::Settings) {
            return;
        }
        let mode = self.fold_form();
        if mode != self.form.mode {
            self.form.mode = mode;
            self.display.redraw();
        }
    }

    fn fold_form(&mut self) -> Mode {
        let Some(utc_ms) = self.readings.live.utc_ms else {
            return Mode::NoClock;
        };
        if !self.counted() {
            return Mode::Reading;
        }
        let key = self.rides_key(utc_ms);
        if let Some((cached, mode)) = self.form.folded
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
            Mode::Form {
                form: daily.form(),
                left_out: daily.unpowered != 0,
            }
        };
        self.form.folded = Some((key, mode));
        mode
    }
}
