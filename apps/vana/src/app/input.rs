//! Physical controls, touch and workout navigation.
//!
//! Buttons keep one role on every screen (see `docs/ux-flows.md`) and go to the
//! focused control, their first responder. Touch goes to what lies under the
//! finger (`hit_test`), through the arbiter in `screens::touch`: it selects,
//! scrolls and changes pages, and never starts, pauses, laps or stops a ride.
use super::Runtime;
use super::mark::MoveReport;
use super::navigation::{Report, To};
use crate::clock::Zone;
use crate::ride::laps::AutoLap;
use crate::ride::log::Status;
use crate::ride::wheel_size::PLAUSIBLE_MM;
use crate::ride::zones::TrainingProfile;
use crate::screens::ride::RidePage;
use crate::screens::ride::summary;
use crate::screens::sensors::Action;
use crate::screens::settings;
use crate::screens::touch::{Control, Hit, Owner, Pull, Swipe, TouchEvent};
use crate::screens::workout::Page;
use crate::sensors::ant::device_type;
use device_api::ant::Ant;
use device_api::input::Role;
use firmware_shell::input::Input;

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
pub const HOLD_GAP_MS: u64 = 250;

impl Runtime {
    pub fn input_active(&self) -> bool {
        self.display.is_active()
    }

    /// An input as the device received it at `received_ms`, handled at `now`.
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
            Input::Touch(point) => {
                self.compose_shown();
                let radar = ant.channel(device_type::RADAR, now).is_some();
                self.compose_shown();
                self.hit_test(point, now, radar)
            }
            _ => Hit::default(),
        };
        let owner = self.touch_owner();
        let events = self
            .navigation
            .touches_mut()
            .feed(input, received, owner, |_| hit);
        // A list always hears how its finger ends, even under a sheet, so it
        // never waits for a lift it will not see.
        for event in &events {
            if let TouchEvent::Pan(list, pan) = *event {
                self.list_pan(list, pan, received);
            }
        }
        if input == Input::Cancel {
            self.mark_input_lost(now, ant);
        }
        if !self.display.is_active() {
            return;
        }
        // The companion repeats reports while a button is held. A report soon
        // after the previous one of that button continues a hold, such as the
        // power-on hold of TopLeft, rather than starting a new press.
        let held = match input {
            Input::Press {
                role: button,
                code: 1,
            } => self.navigation.reported(button, received, HOLD_GAP_MS) == Report::Held,
            _ => false,
        };
        // A power sheet covers every page and takes buttons first; touch never
        // turns the device off.
        if self.power.view().is_some() {
            if let Input::Press {
                role: button,
                code: 1,
            } = input
                && !held
                && received.saturating_sub(self.navigation.last_press()) >= DEBOUNCE_MS
                && let Some(super::composition::Pressed::Asks(action)) = self.composed_press(button)
            {
                self.navigation.pressed(received);
                self.display.redraw();
                self.carry(action, held, (received, now), ant);
            }
            return;
        }
        // The FTP sheet after a test takes the buttons next; touch never
        // answers it.
        if self.ftp_sheet().is_some() {
            if let Input::Press {
                role: button,
                code: 1,
            } = input
                && !held
                && received.saturating_sub(self.navigation.last_press()) >= DEBOUNCE_MS
                && let Some(super::composition::Pressed::Asks(action)) = self.composed_press(button)
            {
                self.navigation.pressed(received);
                self.display.redraw();
                self.carry(action, held, (received, now), ant);
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
        if self.sheet_open() {
            if let Input::Press {
                role: button,
                code: 1,
            } = input
            {
                self.button_press(button, held, received, now, ant);
            }
            return;
        }
        // Buttons and a Cancel reach the sensor menu, so lost input makes
        // MOVE reconfirm the focus before ACT can change a sensor.
        if self.navigation.page() == Page::Sensors {
            self.sensors_input(input, received, now, ant);
            return;
        }
        let Input::Press {
            role: button,
            code: 1,
        } = input
        else {
            return;
        };
        if button != Role::Move {
            self.button_press(button, held, received, now, ant);
            return;
        }
        match self.mark_sees_every_move_report(held, received, now, ant) {
            MoveReport::Held => {}
            MoveReport::Press => {
                self.button_press(button, held, received, now, ant);
                self.mark_after_press(now, ant);
            }
        }
    }

    fn button_press(
        &mut self,
        button: Role,
        held: bool,
        received: u64,
        now: u64,
        ant: &mut impl Ant,
    ) {
        if received.saturating_sub(self.navigation.last_press()) < DEBOUNCE_MS {
            return;
        }
        self.navigation.pressed(received);
        self.display.redraw();
        // The number editor's buttons step and repeat outside the tree.
        if self.editing_number() && !self.sheet_open() {
            self.settings_input(button, now, held);
        } else if let Some(super::composition::Pressed::Asks(action)) = self.composed_press(button)
        {
            self.act(action, held, received, now, ant);
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
        let page = navigation.page() as u32;
        let subview = match navigation.page() {
            // The paused screen is a view of its own over the ride pages.
            Page::Ride => {
                let paused = self.ride.active().status() == Status::Paused;
                let screen = paused && !navigation.pause_hidden();
                navigation.ride_page().index() as u32 | u32::from(screen) << 3
            }
            Page::Summary | Page::PastRide => navigation.summary_page() as u32,
            Page::Sensors => u32::from(self.menu.touch_view()),
            Page::Settings => {
                let section = navigation.sections().section(navigation.settings_field()).0 as u32;
                u32::from(navigation.editing()) | section << 1
            }
            Page::Setup => u32::from(self.preferences.options.setup),
            Page::Boot
            | Page::Format
            | Page::Home
            | Page::Ready
            | Page::EndRide
            | Page::DiscardRide
            | Page::LoseRides
            | Page::Phone
            | Page::Bests
            | Page::Form
            | Page::History
            | Page::Workouts => 0,
        };
        let sheet = u32::from(self.power.view().is_some() || self.ftp_sheet().is_some());
        let dark = u32::from(!self.display.is_active());
        let pulled = u32::from(self.sheet_open());
        Owner(page | subview << 4 | sheet << 12 | dark << 13 | pulled << 14)
    }

    /// What lies under a finger landing at `point`: the control a tap
    /// would act on, the list it would scroll and whether the page swipes.
    pub(super) fn hit_test(&self, point: device_api::input::Point, now: u64, radar: bool) -> Hit {
        if self.sheet_open() {
            return self.sheet_hit(point);
        }
        let (x, y) = (usize::from(point.x), usize::from(point.y));
        let navigation = &self.navigation;
        let paused_screen = self.paused_screen_shown();
        let composed = self.composed_hit(point);
        let key = self.key_at(x, y);
        let control = match navigation.page() {
            // The sensor menu takes its taps itself, as it takes its presses.
            Page::Sensors => Some(Control::Menu(point)),
            _ if composed.is_some() => composed,
            _ if key.is_some() => key,
            Page::Ride if paused_screen => None,
            // The course sign floats over every ride page, a region of the
            // screen rather than a tile of its tree.
            Page::Ride
                if navigation.ride_page() != RidePage::Map && self.sign_at(x, y, now, radar) =>
            {
                Some(Control::Sign)
            }
            // A hold anywhere on the map stops following the course.
            Page::Ride if navigation.ride_page() == RidePage::Map => Some(Control::Map),
            // A screen with a tree acts only through its tiles.
            _ => None,
        };
        let mut hit = Hit {
            control,
            list: None,
            // Settings swipes between sections, out of the editor too.
            swipes: matches!(
                navigation.page(),
                Page::Ride | Page::Summary | Page::PastRide | Page::Settings
            ),
            pull: self.pulls_down(y).then_some(Pull::Down),
        };
        if let Some(list) = self.page_list() {
            let layout = self.list_layout(list);
            if layout.on_page(y) {
                let count = self.list_count(list);
                let offset = self.list_offset(list, now);
                hit.list = Some(list);
                hit.control = layout
                    .card_at(count, offset, x, y)
                    .and_then(|index| self.composed_item(index));
            }
        }
        hit
    }

    fn key_at(&self, x: usize, y: usize) -> Option<Control> {
        let laid = self.composition.laids().get(self.screen_id())?;
        let mut cues = laid.controls().shown(laid.focus());
        cues.find(|cue| cue.target(self.device).contains(x, y))
            .map(|cue| Control::Key(cue.role))
    }

    fn sign_at(&self, x: usize, y: usize, now: u64, radar: bool) -> bool {
        let sign = super::course::sign_of(&self.track, now);
        let page = self.navigation.ride_page();
        let chip = crate::screens::ride::main_chip(self.composition.main());
        crate::screens::sign::on_page(page, sign, radar, self.device, chip)
            .is_some_and(|placed| placed.rect().contains(x, y))
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
            TouchEvent::Hold(Control::Map) => {
                if let Some(action) = self.composed_hold() {
                    self.carry(action, true, (now, now), ant);
                }
            }
            TouchEvent::Hold(_) => {}
            TouchEvent::Swipe(swipe) => self.swiped(swipe),
            TouchEvent::Pull(Pull::Down) => self.open_sheet(),
            TouchEvent::Pull(Pull::Up) => self.close_sheet(),
            TouchEvent::Drag(Control::Level { left, right }, point) => {
                self.drag_level((left, right), point.x, now)
            }
            TouchEvent::Drag(..) => {}
        }
        self.display.redraw();
    }

    /// A tap lifted on `control`: it acts, and takes the focus if it has one.
    fn activate(&mut self, control: Control, now: u64, ant: &mut impl Ant) {
        match control {
            Control::Composed(action @ crate::compose::input::Action::Sheet(_), _) => {
                self.act(action, false, now, now, ant)
            }
            Control::Composed(action, focus) => {
                self.composed_focus(focus);
                self.act(action, false, now, now, ant);
            }
            Control::Key(role) => self.button_press(role, false, now, now, ant),
            Control::Menu(point) => {
                self.refresh_sensor_menu(ant, now);
                let (x, y) = (usize::from(point.x), usize::from(point.y));
                self.compose_shown();
                let laids = self.composition.laids();
                let Some(laid) = laids.get(self.menu.screen()) else {
                    return;
                };
                let action = self.menu.tap_at(laid, self.device, x, y, now);
                self.sensors_action(action, now, ant);
            }
            Control::Sign => self.navigation.go(To::RidePage(RidePage::Map)),
            Control::Lower | Control::Raise | Control::Map | Control::Level { .. } => {}
        }
    }

    /// A sideways swipe: the next or previous ride page, summary page or
    /// Settings section. Changing the page is safe while riding.
    fn swiped(&mut self, swipe: Swipe) {
        let next = swipe == Swipe::Next;
        match self.navigation.page() {
            Page::Ride if next => self.next_ride_page(),
            Page::Ride => self.previous_ride_page(),
            Page::Summary | Page::PastRide if next => self.next_summary_page(1),
            Page::Summary | Page::PastRide => self.previous_summary_page(),
            Page::Settings => {
                let navigation = &mut self.navigation;
                let sections = navigation.sections();
                let (section, _) = sections.section(navigation.settings_field());
                let count = sections.count();
                let section = if next {
                    section + 1
                } else {
                    section + count - 1
                };
                navigation.go(To::Section(sections.first_of(section)));
            }
            _ => {}
        }
    }

    fn sensors_input(&mut self, input: Input, received: u64, now: u64, ant: &mut impl Ant) {
        self.refresh_sensor_menu(ant, now);
        let action = self.menu.input(input, received);
        self.sensors_action(action, now, ant);
    }

    /// Show the sensor pages. With nothing saved they open on NEARBY,
    /// which searches.
    pub(super) fn open_sensors(&mut self, from_setup: bool, ant: &mut impl Ant, now: u64) {
        self.navigation.go(To::Sensors { from_setup });
        self.refresh_sensor_menu(ant, now);
        let action = self.menu.open();
        self.sensors_action(action, now, ant);
    }

    fn sensors_action(&mut self, action: Option<Action>, now: u64, ant: &mut impl Ant) {
        match action {
            Some(Action::Scan) => self.request_search(ant, now),
            Some(Action::StopScan) => self.cancel_search(ant, now),
            Some(action @ (Action::Connect(_) | Action::Disconnect(_))) => {
                self.request_sensor(action, ant, now)
            }
            Some(Action::Calibrate) => self.calibrate(ant, now),
            Some(Action::Ride) => {
                self.cancel_search(ant, now);
                self.after_sensors();
            }
            None => {}
        }
        self.refresh_sensor_menu(ant, now);
        if let Some(Action::Connect(peer)) = action {
            self.menu.added(peer);
        }
        self.display.redraw();
    }

    /// The number editor's buttons: MOVE lowers, ACT raises, holding
    /// repeats, and BACK finishes. Settings' rows take theirs from its tree.
    fn settings_input(&mut self, button: Role, now: u64, held: bool) {
        match button {
            Role::Move => self.adjust_setting(-self.setting_step(), now, held),
            Role::Act => self.adjust_setting(self.setting_step(), now, held),
            Role::Back if !self.setup_edit_finished(now) => self.navigation.go(To::EditDone),
            _ => {}
        }
    }

    /// Act on the selected row: edit a number, step a choice or open a page.
    pub(super) fn act_on_setting(&mut self, now: u64, ant: &mut impl Ant) {
        use settings::{Kind, SettingsAction};
        let row = self.navigation.setting();
        match (row.kind(), row.action()) {
            (Kind::Number, _) => self.navigation.go(To::Edit {
                field: self.navigation.settings_field(),
                wheel_origin: self.preferences.options.wheel,
                from_setup: false,
            }),
            (Kind::Choice, _) => self.next_choice(row, now),
            (_, Some(SettingsAction::Sensors)) => self.open_sensors(false, ant, now),
            (_, Some(SettingsAction::Bests)) => self.open_bests(),
            (_, Some(SettingsAction::Form)) => self.open_form(),
            (_, Some(SettingsAction::ConnectPhone)) => {
                self.enter_phone();
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
                let next = BRIGHTNESS_STEPS
                    .into_iter()
                    .find(|step| *step > display.brightness);
                match (options.auto_brightness, next) {
                    (true, _) => {
                        options.auto_brightness = false;
                        display.brightness = BRIGHTNESS_STEPS[0];
                    }
                    (false, Some(step)) => display.brightness = step,
                    (false, None) => options.auto_brightness = true,
                }
            }
            Row::AutoDim => display.dim_secs = after(&DIM_AFTER_SECS, display.dim_secs),
            Row::AutoPause => options.auto_pause = !options.auto_pause,
            Row::AutoLap => options.auto_lap = after(&AUTO_LAP, options.auto_lap),
            Row::TimeZone => options.zone = after(&ZONES, options.zone),
            Row::WheelMode => options.wheel.manual = !options.wheel.manual,
            Row::PageStats | Row::PageLap | Row::PageClimb | Row::PageMap => {
                if let Some(page) = row.ride_page() {
                    options.deck = options.deck.toggled(page);
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
            Row::Mode => options.mode = options.mode.next(),
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
        i32::from(self.navigation.setting().step())
    }

    /// Move the edited number by `step`, within its range. A repeat from a
    /// held button (`held`) never clears weight or age: only a tap does.
    fn adjust_setting(&mut self, step: i32, now: u64, held: bool) {
        let row = self.navigation.setting();
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
        if self.navigation.setting() == settings::Row::Wheel {
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
            let origin = self
                .navigation
                .edit()
                .map_or(options.wheel, |edit| edit.wheel_origin);
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
        let (value, range) = match self.navigation.setting() {
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
        navigation.go(To::SummaryPage((navigation.summary_page() + step) % pages));
    }

    /// One page back. The FTP offer is reached only going forward, so a
    /// swipe back from the first page lands on the last page every ride has,
    /// where ACT is Done as it is everywhere else.
    fn previous_summary_page(&mut self) {
        let last = summary::pages(self.has_elevation()) - 1;
        let navigation = &mut self.navigation;
        let page = match navigation.summary_page() {
            0 => last,
            page => (page - 1).min(last),
        };
        navigation.go(To::SummaryPage(page));
    }

    /// BottomLeft steps through the ride pages, wrapping after the last.
    /// While paused the paused screen comes first: from it to the page the
    /// rider was on, and from the last page back to it.
    pub(super) fn next_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let pages = self.ride_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden() {
            navigation.go(To::LeavePause {
                wrapped_to: pages.first(),
            });
            return;
        }
        match pages.next(navigation.ride_page()) {
            Some(next) => navigation.go(To::RidePage(next)),
            None if paused => navigation.go(To::PauseWrapped),
            None => navigation.go(To::RidePage(pages.first())),
        }
    }

    /// A swipe back: the mirror of `next_ride_page`.
    pub(super) fn previous_ride_page(&mut self) {
        let paused = self.ride.active().status() == Status::Paused;
        let pages = self.ride_pages();
        let navigation = &mut self.navigation;
        if paused && !navigation.pause_hidden() {
            navigation.go(To::LeavePause {
                wrapped_to: pages.last(),
            });
            return;
        }
        if paused && navigation.ride_page() == pages.first() {
            navigation.go(To::PauseWrapped);
            return;
        }
        let page = pages.preceding(navigation.ride_page());
        navigation.go(To::RidePage(page));
    }
}
