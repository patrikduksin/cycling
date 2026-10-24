//! Physical controls, touch gestures and workout navigation.
//!
//! Buttons keep one role on every screen (see `docs/ui.md`). Touch selects
//! and changes pages; it never starts, pauses, laps or stops a ride.
use super::Runtime;
use crate::ride::log::Status;
use crate::ride::zones::TrainingProfile;
use crate::screens::gesture::{Gesture, Owner};
use crate::screens::home::{Home, Tile};
use crate::screens::ready;
use crate::screens::ride::RidePage;
use crate::screens::ride::summary::PAGES as SUMMARY_PAGES;
use crate::screens::sensors::Action;
use crate::screens::settings;
use crate::screens::workout::{HOME, Page};
use device_api::ant::Ant;
use device_api::input::{Button, Input};

const DEBOUNCE_MS: u64 = 350;
/// Settings choices, in the order acting steps through them.
const BRIGHTNESS_STEPS: [u8; 4] = [25, 50, 75, 100];
const DIM_AFTER_SECS: [u16; 5] = [15, 30, 60, 300, 0];
const AUTO_LAP_KM: [u8; 5] = [0, 1, 2, 5, 10];
/// Held buttons repeat about every 100 ms; a longer gap means a new press.
const HOLD_GAP_MS: u64 = 250;

impl Runtime {
    pub fn input_active(&self) -> bool {
        self.display.is_active()
    }

    pub fn input(&mut self, input: Input, now: u64, ant: &mut impl Ant) {
        self.input_received(input, now, now, ant);
    }

    /// `input` as the device received it at `received_ms`, handled at `now`.
    /// Every input-timing decision (hold continuity, debounce, double-press
    /// windows, press-to-frame and ride-start latency) uses reception, so
    /// time the edge spent queued behind rendering or storage work neither
    /// splits a hold nor hides latency. Ride data keeps the handling time.
    pub fn input_received(&mut self, input: Input, received_ms: u64, now: u64, ant: &mut impl Ant) {
        let received = received_ms.min(now);
        // Every touch edge reaches the recognizer first, whatever owns input,
        // so a sheet or a dark screen can never leave it mid-contact. A
        // contact only completes on the screen it began on.
        let gesture = self
            .navigation
            .gestures
            .feed(input, received, self.touch_owner());
        if !self.display.is_active() {
            return;
        }
        // The companion repeats reports while a button is held. A report soon
        // after the previous one of that button continues a hold, such as the
        // power-on hold of TopLeft, rather than starting a new press.
        let held = match input {
            Input::Button { button, code: 1 } => {
                let last = &mut self.navigation.last_report[button.index().min(2)];
                let held = last.is_some_and(|at| received.saturating_sub(at) < HOLD_GAP_MS);
                *last = Some(received);
                held
            }
            _ => false,
        };
        // A power sheet covers every page and takes buttons first; touch never
        // turns the device off.
        if self.power.view().is_some() {
            if let Input::Button { button, code: 1 } = input
                && !held
                && received.saturating_sub(self.navigation.last_press) >= DEBOUNCE_MS
                && self.power_input(button)
            {
                self.navigation.last_press = received;
                self.display.redraw();
            }
            return;
        }
        // The FTP sheet after a test takes the buttons next; touch never
        // answers it.
        if self.training.ftp_offer.is_some() {
            if let Input::Button { button, code: 1 } = input
                && !held
                && received.saturating_sub(self.navigation.last_press) >= DEBOUNCE_MS
                && self.ftp_input(button, now)
            {
                self.navigation.last_press = received;
                self.display.redraw();
            }
            return;
        }
        // Settings pads act on contact, not on a finished gesture.
        if self.navigation.page == Page::Settings
            && self.navigation.settings_editing
            && let Some(Gesture::Down(point)) = gesture
        {
            self.settings_touch(point, now);
            self.display.redraw();
            return;
        }
        if matches!(input, Input::Touch(_) | Input::Release) {
            if let Some(gesture) = gesture {
                self.gesture(gesture, now, ant);
            }
            return;
        }
        // Buttons and a Cancel reach the sensor menu, so lost input makes
        // MOVE reconfirm the focus before ACT can change a sensor.
        if self.navigation.page == Page::Sensors {
            self.sensors_input(input, received, now, ant);
            return;
        }
        let Input::Button { button, code: 1 } = input else {
            return;
        };
        if received.saturating_sub(self.navigation.last_press) < DEBOUNCE_MS {
            return;
        }
        self.navigation.last_press = received;
        self.display.redraw();
        match self.navigation.page {
            Page::Home => self.home_input(button, received, now, ant, held),
            Page::Ride if button == Button::BottomLeft => {
                self.timing.press(received);
                self.next_ride_page();
            }
            Page::Ready | Page::Ride | Page::EndRide => {
                self.timing.press(received);
                self.ride_press(button, held, received, now);
            }
            Page::Summary if button == Button::BottomLeft => self.next_summary_page(1),
            Page::Summary => self.ride_press(button, held, received, now),
            Page::Phone => self.phone_input(button, held),
            Page::History => self.history_input(button, held),
            Page::PastRide => self.past_ride_input(button, held),
            Page::Settings => self.settings_input(button, now, ant),
            Page::Workouts => self.workouts_input(button, now),
            Page::Setup => self.setup_input(button, now, ant),
            // A press during the brand flash opens Home straight away.
            Page::Boot => self.navigation.page = self.first_page(),
            Page::Sensors => {}
        }
    }

    /// Everything that decides what a touch lands on: the page and its
    /// subview (the ride page, the sensor menu's tiles, detail or replacement
    /// page, Settings' edit mode with its pads), a power sheet, and a dark
    /// screen. Any change cancels a contact in progress.
    ///
    /// Left out on purpose, because touch never acts on them: Home's cursor
    /// and Settings' selected field (targets have fixed bounds), the end-ride
    /// confirm window (touch never controls a ride) and toasts (not
    /// tappable).
    fn touch_owner(&self) -> Owner {
        let navigation = &self.navigation;
        let page = navigation.page as u32;
        let subview = match navigation.page {
            // The paused screen is a view of its own over the ride pages.
            Page::Ride => {
                let paused = self.ride.active().status() == Status::Paused;
                let screen = paused && !navigation.pause_hidden;
                navigation.ride_page.index() as u32 | u32::from(screen) << 3
            }
            Page::Summary | Page::PastRide => navigation.summary_page as u32,
            // Cards scroll with the focus, so a tap belongs to one list view.
            Page::History => self.history.cursor as u32,
            Page::Sensors => u32::from(self.menu.touch_view()),
            Page::Settings => {
                let section = settings::section(navigation.settings_field).0 as u32;
                u32::from(navigation.settings_editing) | section << 1
            }
            Page::Setup => u32::from(self.preferences.options.setup),
            Page::Boot
            | Page::Home
            | Page::Ready
            | Page::EndRide
            | Page::Phone
            | Page::Workouts => 0,
        };
        let sheet = u32::from(self.power.view().is_some() || self.training.ftp_offer.is_some());
        let dark = u32::from(!self.display.is_active());
        Owner(page | subview << 4 | sheet << 12 | dark << 13)
    }

    fn gesture(&mut self, gesture: Gesture, now: u64, ant: &mut impl Ant) {
        match (self.navigation.page, gesture) {
            (Page::Home, Gesture::Tap(point)) => {
                let Some(tile) = Home::tile_at(usize::from(point.x), usize::from(point.y)) else {
                    return;
                };
                self.navigation.home_cursor = match tile {
                    Tile::Ride => 0,
                    Tile::Sensors => 1,
                    Tile::Workouts => 2,
                    Tile::Settings => 3,
                    Tile::Rides => 4,
                };
                self.open_home_tile(now, ant);
            }
            // A tap on the sensors line of Ready pairs sensors.
            // A tap on the workout line under the headline changes it.
            (Page::Ready, Gesture::Tap(point)) if ready::riding_at(usize::from(point.y)) => {
                self.navigation.page = Page::Workouts;
            }
            (Page::Ready, Gesture::Tap(point)) => {
                if ready::line_at(usize::from(point.y)) != Some(ready::Line::Sensors) {
                    return;
                }
                self.navigation.page = Page::Sensors;
                self.request_search(ant, now);
            }
            // A tap on the code switches codes; after a session ended, any
            // tap tries again.
            (Page::Phone, Gesture::Tap(point)) => {
                let on_code =
                    crate::screens::phone::on_code(usize::from(point.x), usize::from(point.y));
                if on_code || self.phone_status().is_none() {
                    self.phone_touch();
                }
            }
            (Page::Summary | Page::PastRide, Gesture::Next) => self.next_summary_page(1),
            (Page::Summary | Page::PastRide, Gesture::Previous) => {
                self.next_summary_page(SUMMARY_PAGES - 1)
            }
            (Page::History, Gesture::Tap(point)) => {
                let (x, y) = (usize::from(point.x), usize::from(point.y));
                let count = self.history_count();
                if let Some(index) =
                    crate::screens::history::row_at(count, self.history.cursor, x, y)
                {
                    self.open_past_ride(index);
                }
            }
            (Page::Sensors, Gesture::Tap(point)) => {
                self.refresh_sensor_menu(ant, now);
                let action = self.menu.tap(usize::from(point.x), usize::from(point.y));
                self.sensors_action(action, now, ant);
            }
            (Page::Workouts, Gesture::Tap(point)) => {
                let picker = self.picker();
                let Some(row) = picker.row_at(usize::from(point.x), usize::from(point.y)) else {
                    return;
                };
                self.choose_workout(row, now);
            }
            // The back-to-start page's pad follows the track back, or stops.
            (Page::Ride, Gesture::Tap(point))
                if self.navigation.ride_page == RidePage::Back
                    && !(self.ride.active().status() == Status::Paused
                        && !self.navigation.pause_hidden) =>
            {
                let radar = crate::sensors::ant::device_type::RADAR;
                let centred = ant.channel(radar, now).is_none();
                if !crate::screens::ride::back::pad_at(
                    usize::from(point.x),
                    usize::from(point.y),
                    centred,
                ) {
                    return;
                }
                self.toggle_follow(now);
            }
            // Changing the page is safe while riding; ride control is not.
            (Page::Ride, Gesture::Next) => self.next_ride_page(),
            (Page::Ride, Gesture::Previous) => self.previous_ride_page(),
            (Page::Setup, Gesture::Tap(point)) => {
                self.setup_tap(usize::from(point.x), usize::from(point.y), now, ant)
            }
            // Adding a minute to a step changes no ride state, so a tap may.
            (Page::Ride, Gesture::Tap(point))
                if self.navigation.ride_page == RidePage::Workout
                    && crate::screens::ride::EXTEND_ROWS.contains(&usize::from(point.y)) =>
            {
                self.extend_step(now)
            }
            (Page::Settings, Gesture::Tap(point)) => {
                let navigation = &self.navigation;
                if let Some(settings::Target::Row(row)) = settings::target(
                    usize::from(point.x),
                    usize::from(point.y),
                    navigation.settings_field,
                    navigation.settings_editing,
                ) {
                    self.navigation.settings_field = row;
                    self.act_on_setting(now, ant);
                }
            }
            (Page::Settings, Gesture::Next | Gesture::Previous) => {
                let navigation = &mut self.navigation;
                let (section, _) = settings::section(navigation.settings_field);
                let count = settings::SECTIONS.len();
                let section = if gesture == Gesture::Next {
                    section + 1
                } else {
                    section + count - 1
                };
                navigation.settings_field = settings::first_of(section);
                navigation.settings_editing = false;
            }
            _ => return,
        }
        self.display.redraw();
    }

    fn sensors_input(&mut self, input: Input, received: u64, now: u64, ant: &mut impl Ant) {
        self.refresh_sensor_menu(ant, now);
        let action = self.menu.input(input, received);
        self.sensors_action(action, now, ant);
    }

    fn sensors_action(&mut self, action: Option<Action>, now: u64, ant: &mut impl Ant) {
        match action {
            Some(Action::Scan) => self.request_search(ant, now),
            Some(action @ (Action::Connect(_) | Action::Disconnect(_))) => {
                self.request_sensor(action, ant, now)
            }
            Some(Action::Ride) => {
                self.cancel_search(ant, now);
                self.navigation.page = self.after_sensors();
            }
            None => {}
        }
        self.refresh_sensor_menu(ant, now);
        if let Some(Action::Connect(peer)) = action {
            self.menu.focus_type(peer.device_type);
        }
        self.display.redraw();
    }

    /// MOVE steps the focused card, ACT opens it and BACK asks to turn off.
    fn home_input(
        &mut self,
        button: Button,
        received: u64,
        now: u64,
        ant: &mut impl Ant,
        held: bool,
    ) {
        match button {
            Button::BottomLeft => {
                self.navigation.home_cursor = (self.navigation.home_cursor + 1) % HOME.len()
            }
            Button::BottomRight => self.open_home_tile(now, ant),
            // BACK on Home asks to turn off; Home is never reachable mid-ride.
            Button::TopLeft if !held => self.open_power_sheet(received),
            _ => {}
        }
    }

    fn open_home_tile(&mut self, now: u64, ant: &mut impl Ant) {
        self.navigation.page_since = now;
        match self.navigation.home_cursor {
            0 => self.navigation.page = Page::Ready,
            1 => {
                self.navigation.page = Page::Sensors;
                self.request_search(ant, now);
            }
            2 => self.navigation.page = Page::Workouts,
            4 => self.open_history(),
            _ => {
                self.navigation.page = Page::Settings;
                self.navigation.settings_field = 0;
                self.navigation.settings_editing = false;
            }
        }
    }

    /// Choosing a row: BottomLeft moves to the next, BottomRight acts on it,
    /// TopLeft goes Home. Editing a number: BottomLeft lowers and
    /// BottomRight raises it by `settings::STEP`, TopLeft finishes.
    fn settings_input(&mut self, button: Button, now: u64, ant: &mut impl Ant) {
        let navigation = &mut self.navigation;
        if !navigation.settings_editing {
            match button {
                Button::BottomLeft => {
                    navigation.settings_field = (navigation.settings_field + 1) % settings::ROWS
                }
                Button::BottomRight => self.act_on_setting(now, ant),
                Button::TopLeft => navigation.page = Page::Home,
                _ => {}
            }
            return;
        }
        match button {
            Button::BottomLeft => self.adjust_setting(-i32::from(settings::STEP), now),
            Button::BottomRight => self.adjust_setting(i32::from(settings::STEP), now),
            Button::TopLeft if !self.setup_edit_finished(now) => {
                self.navigation.settings_editing = false
            }
            _ => {}
        }
    }

    /// Act on the selected row: edit a number, step a choice or open a page.
    fn act_on_setting(&mut self, now: u64, ant: &mut impl Ant) {
        use settings::{Kind, Row, SettingsAction};
        let row = Row::at(self.navigation.settings_field);
        match (row.kind(), row.action()) {
            (Kind::Number, _) => self.navigation.settings_editing = true,
            (Kind::Choice, _) => self.next_choice(row, now),
            (_, Some(SettingsAction::Sensors)) => {
                self.navigation.page = Page::Sensors;
                self.navigation.page_since = now;
                self.request_search(ant, now);
            }
            (_, Some(SettingsAction::ConnectPhone)) => {
                self.enter_phone();
                self.navigation.page_since = now;
            }
            _ => {}
        }
    }

    /// Step a choice row to its next value, wrapping.
    fn next_choice(&mut self, row: settings::Row, now: u64) {
        use settings::Row;
        /// The choice after `current`, or the first when it is none of them.
        fn after<T: Copy + PartialEq>(choices: &[T], current: T) -> T {
            let at = choices.iter().position(|choice| *choice == current);
            choices[at.map_or(0, |at| (at + 1) % choices.len())]
        }
        let mut options = self.preferences.options;
        let mut display = self.preferences.display;
        match row {
            Row::Brightness => {
                // The next step up, so a level set elsewhere moves on too.
                display.brightness = BRIGHTNESS_STEPS
                    .into_iter()
                    .find(|step| *step > display.brightness)
                    .unwrap_or(BRIGHTNESS_STEPS[0]);
            }
            Row::AutoDim => display.dim_secs = after(&DIM_AFTER_SECS, display.dim_secs),
            Row::AutoPause => options.auto_pause = !options.auto_pause,
            Row::AutoLap => options.auto_lap_km = after(&AUTO_LAP_KM, options.auto_lap_km),
            Row::Sounds => options.sounds = !options.sounds,
            Row::Daylight => {
                use super::preferences::Daylight;
                options.daylight = after(
                    &[Daylight::Auto, Daylight::Day, Daylight::Night],
                    options.daylight,
                )
            }
            Row::Theme => {
                use super::preferences::Theme;
                options.theme = after(&[Theme::Dark, Theme::Light, Theme::Auto], options.theme)
            }
            Row::RadarAlerts
            | Row::FuelAlert
            | Row::CadenceLow
            | Row::HeartHigh
            | Row::HeartLow
            | Row::PowerHigh
            | Row::PowerLow => {
                use crate::ride::alerts as a;
                let mut alerts = self.alert_settings();
                match row {
                    Row::RadarAlerts => alerts.radar = !alerts.radar,
                    Row::FuelAlert => {
                        alerts.fuel_every_min = after(&a::FUEL_MINUTES, alerts.fuel_every_min)
                    }
                    Row::CadenceLow => {
                        alerts.cadence_low_rpm = after(&a::CADENCE_LOW_RPM, alerts.cadence_low_rpm)
                    }
                    Row::HeartHigh => {
                        alerts.heart_high_bpm = after(&a::HEART_HIGH_BPM, alerts.heart_high_bpm)
                    }
                    Row::HeartLow => {
                        alerts.heart_low_bpm = after(&a::HEART_LOW_BPM, alerts.heart_low_bpm)
                    }
                    Row::PowerHigh => {
                        alerts.power_high_w = after(&a::POWER_HIGH_W, alerts.power_high_w)
                    }
                    _ => alerts.power_low_w = after(&a::POWER_LOW_W, alerts.power_low_w),
                }
                options.alerts = alerts.encode();
            }
            _ => return,
        }
        if display != self.preferences.display {
            self.set_display(display, now);
        }
        if options != self.preferences.options {
            self.set_options(options, now);
        }
    }

    /// A touch on a pad lowers or raises the edited number. Each acts once,
    /// when the contact begins.
    fn settings_touch(&mut self, point: device_api::input::Point, now: u64) {
        let navigation = &self.navigation;
        match settings::target(
            usize::from(point.x),
            usize::from(point.y),
            navigation.settings_field,
            navigation.settings_editing,
        ) {
            Some(settings::Target::Lower) => self.adjust_setting(-i32::from(settings::STEP), now),
            Some(settings::Target::Raise) => self.adjust_setting(i32::from(settings::STEP), now),
            _ => {}
        }
    }

    /// Move the edited number by `step`, within its range.
    fn adjust_setting(&mut self, step: i32, now: u64) {
        let mut profile = self.preferences.profile;
        let (value, range) = match settings::Row::at(self.navigation.settings_field) {
            settings::Row::Ftp => (&mut profile.ftp_watts, TrainingProfile::FTP_WATTS),
            settings::Row::Threshold => (&mut profile.lthr_bpm, TrainingProfile::LTHR_BPM),
            _ => return,
        };
        *value = (i32::from(*value) + step)
            .clamp(i32::from(*range.start()), i32::from(*range.end())) as u16;
        self.preferences.set(profile, now);
    }

    pub(super) fn next_summary_page(&mut self, step: usize) {
        let navigation = &mut self.navigation;
        navigation.summary_page = (navigation.summary_page + step) % SUMMARY_PAGES;
    }

    /// BottomLeft steps through the ride pages, wrapping after the last.
    /// While paused the paused screen comes first: from it to the page the
    /// rider was on, and from the last page back to it.
    fn next_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let workout = self.workout_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden {
            navigation.pause_hidden = true;
            if core::mem::take(&mut navigation.pause_wrapped) {
                navigation.ride_page_chosen(RidePage::pages(workout)[0]);
            }
            return;
        }
        match navigation.ride_page.next_in(workout) {
            Some(next) => navigation.ride_page_chosen(next),
            None if paused => {
                navigation.pause_hidden = false;
                navigation.pause_wrapped = true;
            }
            None => navigation.ride_page_chosen(RidePage::pages(workout)[0]),
        }
    }

    /// A swipe back: the mirror of `next_ride_page`.
    pub(super) fn previous_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let workout = self.workout_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden {
            navigation.pause_hidden = true;
            if core::mem::take(&mut navigation.pause_wrapped) {
                let pages = RidePage::pages(workout);
                navigation.ride_page_chosen(pages[pages.len() - 1]);
            }
            return;
        }
        if paused && navigation.ride_page == RidePage::pages(workout)[0] {
            navigation.pause_hidden = false;
            navigation.pause_wrapped = true;
            return;
        }
        let page = navigation.ride_page.preceding_in(workout);
        navigation.ride_page_chosen(page);
    }
}
