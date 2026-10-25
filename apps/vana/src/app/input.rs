//! Physical controls, touch and workout navigation.
//!
//! Buttons keep one role on every screen (see `docs/ui.md`) and go to the
//! focused control, their first responder. Touch goes to what lies under the
//! finger (`hit_test`), through the arbiter in `screens::touch`: it selects,
//! scrolls and changes pages, and never starts, pauses, laps or stops a ride.
use super::Runtime;
use crate::clock::Zone;
use crate::ride::laps::AutoLap;
use crate::ride::log::Status;
use crate::ride::wheel_size::PLAUSIBLE_MM;
use crate::ride::zones::TrainingProfile;
use crate::screens::home::{Home, Tile};
use crate::screens::ready;
use crate::screens::ride::RidePage;
use crate::screens::ride::summary;
use crate::screens::sensors::Action;
use crate::screens::settings;
use crate::screens::touch::{Control, Hit, List, Owner, Swipe, TouchEvent};
use crate::screens::workout::{HOME, Page};
use device_api::ant::Ant;
use device_api::input::{Button, Input};

const DEBOUNCE_MS: u64 = 350;
/// Settings choices, in the order acting steps through them.
const BRIGHTNESS_STEPS: [u8; 4] = [25, 50, 75, 100];
const DIM_AFTER_SECS: [u16; 5] = [15, 30, 60, 300, 0];
const AUTO_LAP: [AutoLap; 6] = [
    AutoLap::Off,
    AutoLap::Every(1),
    AutoLap::Every(2),
    AutoLap::Every(5),
    AutoLap::Every(10),
    AutoLap::AtStart,
];
/// UTC-3 is Magallanes and Aysén all year.
const ZONES: [Zone; 3] = [Zone::Chile, Zone::utc(-3), Zone::utc(-4)];
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
        // Every touch edge reaches the arbiter first, whatever owns input,
        // so a sheet or a dark screen can never leave it mid-contact. A
        // contact only completes on the screen it began on.
        let hit = match input {
            Input::Touch(point) => self.hit_test(point, now, ant),
            _ => Hit::default(),
        };
        let owner = self.touch_owner();
        let events = self
            .navigation
            .touches
            .feed(input, received, owner, |_| hit);
        // A list always hears how its finger ends, even under a sheet, so it
        // never waits for a lift it will not see.
        for event in &events {
            if let TouchEvent::Pan(list, pan) = *event {
                self.list_pan(list, pan, received);
            }
        }
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
        if matches!(input, Input::Touch(_) | Input::Release | Input::Cancel) {
            for event in events {
                self.touched(event, now, ant);
            }
            if matches!(input, Input::Touch(_) | Input::Release) {
                return;
            }
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
            // The FTP offer's answer, then the press ends the summary as on any page.
            Page::Summary if self.offer_shown() => {
                if !held {
                    self.answer_ftp_offer(button, now);
                }
                self.ride_press(button, held, received, now);
            }
            Page::Summary => self.ride_press(button, held, received, now),
            Page::Bests => self.bests_input(button, held),
            Page::Form => self.form_input(button, held),
            Page::Phone => self.phone_input(button, held),
            Page::History => self.history_input(button, held, now),
            Page::PastRide => self.past_ride_input(button, held),
            Page::Settings => self.settings_input(button, now, ant, held),
            Page::Workouts => self.workouts_input(button, now, held),
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
            | Page::Bests
            | Page::Form
            | Page::History
            | Page::Workouts => 0,
        };
        let sheet = u32::from(self.power.view().is_some() || self.training.ftp_offer.is_some());
        let dark = u32::from(!self.display.is_active());
        Owner(page | subview << 4 | sheet << 12 | dark << 13)
    }

    /// What lies under a finger landing at `point`: the control a tap
    /// would act on, the list it would scroll and whether the page swipes.
    fn hit_test(&self, point: device_api::input::Point, now: u64, ant: &impl Ant) -> Hit {
        let (x, y) = (usize::from(point.x), usize::from(point.y));
        let navigation = &self.navigation;
        let paused_screen = self.navigation.page == Page::Ride
            && self.ride.active().status() == Status::Paused
            && !navigation.pause_hidden;
        let control = match navigation.page {
            Page::Home => Home::tile_at(x, y).map(Control::Tile),
            Page::Ready if ready::riding_at(y) => Some(Control::ReadyWorkout),
            Page::Ready => {
                (ready::line_at(y) == Some(ready::Line::Sensors)).then_some(Control::ReadySensors)
            }
            // A tap on the code switches codes; after a session ended, any
            // tap tries again.
            Page::Phone => (crate::screens::phone::on_code(x, y) || self.phone_status().is_none())
                .then_some(Control::Phone),
            Page::Sensors => Some(Control::Menu(point)),
            Page::Setup => self
                .setup_step()
                .and_then(|step| crate::screens::setup::target(x, y, step))
                .map(Control::Setup),
            Page::Settings if navigation.settings_editing => {
                match settings::target(x, y, navigation.settings_field, true, 0) {
                    Some(settings::Target::Lower) => Some(Control::Lower),
                    Some(settings::Target::Raise) => Some(Control::Raise),
                    _ => None,
                }
            }
            Page::Ride if paused_screen => None,
            // The back-to-start page's pad follows the track back, or stops.
            Page::Ride if navigation.ride_page == RidePage::Back => {
                let radar = crate::sensors::ant::device_type::RADAR;
                let centred = ant.channel(radar, now).is_none();
                crate::screens::ride::back::pad_at(x, y, centred).then_some(Control::BackPad)
            }
            Page::Ride if navigation.ride_page == RidePage::Map => Some(Control::Map),
            // Adding a minute to a step changes no ride state, so a tap may.
            Page::Ride
                if navigation.ride_page == RidePage::Workout
                    && crate::screens::ride::EXTEND_ROWS.contains(&y) =>
            {
                Some(Control::ExtendStep)
            }
            _ => None,
        };
        let mut hit = Hit {
            control,
            list: None,
            // Settings swipes between sections, out of the editor too.
            swipes: matches!(
                navigation.page,
                Page::Ride | Page::Summary | Page::PastRide | Page::Settings
            ),
        };
        if let Some(list) = self.page_list() {
            let layout = Self::list_layout(list);
            if layout.on_page(y) {
                let count = self.list_count(list);
                let offset = self.list_offset(list, now);
                hit.list = Some(list);
                hit.control = layout.card_at(count, offset, x, y).map(|index| match list {
                    List::Rides => Control::Ride(index),
                    List::Workouts => Control::Workout(index),
                    List::Settings => {
                        Control::Setting(settings::rows(navigation.settings_field).1 + index)
                    }
                });
            }
        }
        hit
    }

    /// One event of a contact on the page it began on.
    fn touched(&mut self, event: TouchEvent, now: u64, ant: &mut impl Ant) {
        match event {
            TouchEvent::Pan(..) => return,
            TouchEvent::Pressed(_) | TouchEvent::Released(_) => {}
            TouchEvent::Activated(control) => self.activate(control, now, ant),
            TouchEvent::Contact(Control::Lower) => {
                self.adjust_setting(-self.setting_step(), now, false)
            }
            TouchEvent::Contact(Control::Raise) => {
                self.adjust_setting(self.setting_step(), now, false)
            }
            TouchEvent::Contact(_) => {}
            TouchEvent::Hold(Control::Map)
                if self.navigation.page == Page::Ride
                    && self.navigation.ride_page == RidePage::Map =>
            {
                self.stop_following_from_map(now);
            }
            TouchEvent::Hold(_) => {}
            TouchEvent::Swipe(swipe) => self.swiped(swipe),
        }
        self.display.redraw();
    }

    /// A tap lifted on `control`: it acts, and takes the focus if it has one.
    fn activate(&mut self, control: Control, now: u64, ant: &mut impl Ant) {
        match control {
            Control::Tile(tile) => {
                self.navigation.home_cursor = match tile {
                    Tile::Ride => 0,
                    Tile::Sensors => 1,
                    Tile::Workouts => 2,
                    Tile::Settings => 3,
                    Tile::Rides => 4,
                };
                self.open_home_tile(now, ant);
            }
            Control::ReadyWorkout => {
                self.navigation.page = Page::Workouts;
                self.show_focus(List::Workouts, now);
            }
            Control::ReadySensors => {
                self.navigation.page = Page::Sensors;
                self.request_search(ant, now);
            }
            Control::Phone => self.phone_touch(),
            Control::Ride(index) => self.open_past_ride(index),
            Control::Workout(index) => self.choose_workout(index, now),
            Control::Setting(field) => {
                self.navigation.settings_field = field;
                self.act_on_setting(now, ant);
            }
            Control::Setup(card) => self.setup_card(card, now, ant),
            Control::BackPad => self.toggle_follow(now),
            Control::ExtendStep => self.extend_step(now),
            Control::Menu(point) => {
                self.refresh_sensor_menu(ant, now);
                let action = self.menu.tap(usize::from(point.x), usize::from(point.y));
                self.sensors_action(action, now, ant);
            }
            Control::Lower | Control::Raise | Control::Map => {}
        }
    }

    /// A sideways swipe: the next or previous ride page, summary page or
    /// Settings section. Changing the page is safe while riding.
    fn swiped(&mut self, swipe: Swipe) {
        let next = swipe == Swipe::Next;
        match self.navigation.page {
            Page::Ride if next => self.next_ride_page(),
            Page::Ride => self.previous_ride_page(),
            Page::Summary | Page::PastRide if next => self.next_summary_page(1),
            Page::Summary | Page::PastRide => self.previous_summary_page(),
            Page::Settings => {
                let navigation = &mut self.navigation;
                let (section, _) = settings::section(navigation.settings_field);
                let count = settings::SECTIONS.len();
                let section = if next {
                    section + 1
                } else {
                    section + count - 1
                };
                navigation.settings_field = settings::first_of(section);
                navigation.settings_editing = false;
                navigation.settings_scroll = Default::default();
            }
            _ => {}
        }
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
            Some(Action::Calibrate) => self.calibrate(ant, now),
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
            2 => {
                self.navigation.page = Page::Workouts;
                self.show_focus(List::Workouts, now);
            }
            4 => self.open_history(),
            _ => {
                self.navigation.page = Page::Settings;
                self.navigation.settings_field = 0;
                self.navigation.settings_editing = false;
                self.navigation.settings_scroll = Default::default();
            }
        }
    }

    /// Choosing a row: BottomLeft moves to the next, BottomRight acts on it,
    /// TopLeft goes Home. Editing a number: BottomLeft lowers and
    /// BottomRight raises it by `settings::STEP`, TopLeft finishes.
    fn settings_input(&mut self, button: Button, now: u64, ant: &mut impl Ant, held: bool) {
        if !self.navigation.settings_editing {
            match button {
                Button::BottomLeft => self.settings_move(now),
                Button::BottomRight if self.list_act(List::Settings, now) => {
                    self.act_on_setting(now, ant)
                }
                Button::TopLeft => self.navigation.page = Page::Home,
                _ => {}
            }
            return;
        }
        match button {
            Button::BottomLeft => self.adjust_setting(-self.setting_step(), now, held),
            Button::BottomRight => self.adjust_setting(self.setting_step(), now, held),
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
            (Kind::Number, _) => {
                self.navigation.settings_editing = true;
                self.navigation.wheel_edit = Some(self.preferences.options.wheel);
            }
            (Kind::Choice, _) => self.next_choice(row, now),
            (_, Some(SettingsAction::Sensors)) => {
                self.navigation.page = Page::Sensors;
                self.navigation.page_since = now;
                self.request_search(ant, now);
            }
            (_, Some(SettingsAction::Bests)) => self.open_bests(now),
            (_, Some(SettingsAction::Form)) => self.open_form(now),
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
            Row::AutoLap => options.auto_lap = after(&AUTO_LAP, options.auto_lap),
            Row::TimeZone => options.zone = after(&ZONES, options.zone),
            Row::WheelMode => options.wheel.manual = !options.wheel.manual,
            Row::PageStats | Row::PageLap | Row::PageClimb | Row::PageMap | Row::PageBack => {
                if let Some(page) = row.ride_page() {
                    options.ride_pages = options.ride_pages.toggled(page);
                }
            }
            Row::MainLeft => options.main_slots.left = options.main_slots.left.next(),
            Row::MainRight => options.main_slots.right = options.main_slots.right.next(),
            Row::Sex => options.rider = options.rider.next_sex(),
            Row::Sounds => options.sounds = !options.sounds,
            Row::SunsetAlert => options.sunset_alert = !options.sunset_alert,
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

    /// What one press moves the edited number.
    fn setting_step(&self) -> i32 {
        i32::from(settings::Row::at(self.navigation.settings_field).step())
    }

    /// Move the edited number by `step`, within its range. A repeat from a
    /// held button (`held`) never clears weight or age: only a tap does.
    fn adjust_setting(&mut self, step: i32, now: u64, held: bool) {
        let row = settings::Row::at(self.navigation.settings_field);
        if matches!(row, settings::Row::Weight | settings::Row::Age) {
            let mut options = self.preferences.options;
            options.rider = if row == settings::Row::Weight {
                options.rider.step_weight(step, !held)
            } else {
                options.rider.step_age(step, !held)
            };
            self.set_options(options, now);
            return;
        }
        if settings::Row::at(self.navigation.settings_field) == settings::Row::Wheel {
            // The next multiple of five mm in the direction of the press, so
            // common sizes are reachable from any start. Setting the size by
            // hand takes it out of the calibration's hands, unless the edit
            // comes back to where it began.
            let mut options = self.preferences.options;
            let current = i32::from(options.wheel.size_mm());
            let grid = i32::from(settings::STEP);
            let target = if step > 0 {
                (current / grid + 1) * grid
            } else {
                ((current + grid - 1) / grid - 1) * grid
            };
            let range = PLAUSIBLE_MM;
            let size = target.clamp(i32::from(*range.start()), i32::from(*range.end())) as u16;
            let origin = self.navigation.wheel_edit.unwrap_or(options.wheel);
            options.wheel = if size == origin.size_mm() {
                origin
            } else {
                super::preferences::Wheel {
                    mm: Some(size),
                    manual: true,
                }
            };
            self.set_options(options, now);
            return;
        }
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
        let pages = self.summary_pages();
        let navigation = &mut self.navigation;
        navigation.summary_page = (navigation.summary_page + step) % pages;
    }

    /// One page back. The FTP offer is reached only going forward, so a
    /// swipe back from the first page lands on the last page every ride has,
    /// where ACT is Done as it is everywhere else.
    fn previous_summary_page(&mut self) {
        let last = summary::pages(self.has_elevation()) - 1;
        let navigation = &mut self.navigation;
        navigation.summary_page = match navigation.summary_page {
            0 => last,
            page => (page - 1).min(last),
        };
    }

    /// BottomLeft steps through the ride pages, wrapping after the last.
    /// While paused the paused screen comes first: from it to the page the
    /// rider was on, and from the last page back to it.
    fn next_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let pages = self.ride_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden {
            navigation.pause_hidden = true;
            if core::mem::take(&mut navigation.pause_wrapped) {
                navigation.ride_page_chosen(pages.first());
            }
            return;
        }
        match pages.next(navigation.ride_page) {
            Some(next) => navigation.ride_page_chosen(next),
            None if paused => {
                navigation.pause_hidden = false;
                navigation.pause_wrapped = true;
            }
            None => navigation.ride_page_chosen(pages.first()),
        }
    }

    /// A swipe back: the mirror of `next_ride_page`.
    pub(super) fn previous_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let pages = self.ride_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden {
            navigation.pause_hidden = true;
            if core::mem::take(&mut navigation.pause_wrapped) {
                navigation.ride_page_chosen(pages.last());
            }
            return;
        }
        if paused && navigation.ride_page == pages.first() {
            navigation.pause_hidden = false;
            navigation.pause_wrapped = true;
            return;
        }
        let page = pages.preceding(navigation.ride_page);
        navigation.ride_page_chosen(page);
    }
}
