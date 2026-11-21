use super::climb_page::{self, Follow, Road};
use super::cues::Cue;
use super::preferences::Wheel;
use super::presentation::TOAST_MS;
use crate::ride::log::Status;
use crate::screens::chrome::{Toast, Tone};
use crate::screens::kinetic::Kinetic;
use crate::screens::ride::RidePage;
use crate::screens::settings;
use crate::screens::touch::Touches;
use crate::screens::workout::{Page, Text, View};
use device_api::input::Role;
use firmware_services::why;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Edit {
    pub(super) wheel_origin: Wheel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pause {
    pub(super) hidden: bool,
    pub(super) wrapped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Report {
    Pressed,
    Held,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Restore {
    pub(super) ride: (RidePage, Follow),
    pub(super) pause: Pause,
    pub(super) summary_page: usize,
}

pub(super) enum To {
    Page(Page),
    Settings,
    Section(usize),
    Field(usize),
    Edit {
        field: usize,
        wheel_origin: Wheel,
        from_setup: bool,
    },
    EditDone,
    Setup(Page),
    Sensors {
        from_setup: bool,
    },
    LeaveSensors(Page),
    Ride,
    Summary {
        news: Option<Text<24>>,
    },
    Home,
    PastRide,
    Phone,
    PhoneCode,
    PhoneJoined,
    RidePage(RidePage),
    LeavePause {
        wrapped_to: RidePage,
    },
    PauseWrapped,
    Road {
        road: Road,
        page_on: bool,
    },
    WorkoutDone,
    RideStarted,
    Restore(Restore),
    SummaryPage(usize),
}

pub(super) struct Navigation {
    page: Page,
    /// The ride page shown while `page` is the ride screen.
    ride_page: RidePage,
    settings_field: usize,
    sections: settings::Sections,
    /// Set only on Settings.
    edit: Option<Edit>,
    /// The Settings editor was opened from setup and returns there.
    setup_return: bool,
    /// The sensor menu was opened from setup and returns there.
    sensors_return: bool,
    /// Connect phone shows the join code (0) or the page code (1).
    phone_step: u8,
    /// The screen already moved to the page code when a phone joined.
    phone_advanced: bool,
    /// Whether the climb page came up by itself, and where it goes back to
    /// (`climb_page`).
    climb: Follow,
    /// The road the last pass saw.
    road: Road,
    last_press: u64,
    /// When VANA drew its first frame; the brand flash counts from here.
    first_frame: Option<u64>,
    /// Latest report of each button, to tell a held button from a new press.
    last_report: [Option<u64>; 3],
    /// The finger on the screen, if any, and what it is bound to.
    touches: Touches,
    /// The Settings section's scroll.
    settings_scroll: Kinetic,
    /// A confirmation or hint and when it expires.
    toast: Option<(Toast, u64)>,
    /// The recorder state the last frame announced.
    announced: Status,
    /// When the ride was last paused, for the paused screen's timer.
    paused_since: Option<u64>,
    pause_hidden: bool,
    /// The ride page a ride opens on.
    first_page: RidePage,
    /// The paused screen came back by paging past either end of the ride
    /// pages, so MOVE continues at the first page and a swipe back at the
    /// last, rather than at the page the rider left.
    pause_wrapped: bool,
    /// The summary page shown after a ride.
    summary_page: usize,
    /// A sound the next `alert` plays.
    chime: Option<Cue>,
    /// The personal bests the ride set, announced when its summary opens.
    best_news: Option<Text<24>>,
    /// The bests toast on screen, so leaving the summary can take it away.
    best_shown: Option<Text<24>>,
}

state!(Navigation {
    page,
    ride_page,
    settings_field,
    sections,
    edit,
    setup_return,
    sensors_return,
    phone_step,
    phone_advanced,
    climb,
    road,
    last_press,
    first_frame,
    last_report,
    touches,
    settings_scroll,
    toast,
    announced,
    paused_since,
    pause_hidden,
    first_page,
    pause_wrapped,
    summary_page,
    chime,
    best_news,
    best_shown,
});

impl Default for Navigation {
    fn default() -> Self {
        Self {
            page: Page::Boot,
            ride_page: RidePage::Main,
            settings_field: 0,
            sections: settings::Sections::All,
            edit: None,
            setup_return: false,
            sensors_return: false,
            phone_step: 0,
            phone_advanced: false,
            climb: Default::default(),
            road: Default::default(),
            last_press: 0,
            first_frame: None,
            last_report: [None; 3],
            touches: Touches::default(),
            settings_scroll: Kinetic::default(),
            toast: None,
            announced: Status::Ready,
            paused_since: None,
            pause_hidden: false,
            first_page: RidePage::Main,
            pause_wrapped: false,
            summary_page: 0,
            chime: None,
            best_news: None,
            best_shown: None,
        }
    }
}

impl Navigation {
    pub(super) fn go(&mut self, to: To) {
        let (from, ride_from) = (self.page, self.ride_page);
        match to {
            To::Page(page) => self.page = page,
            To::Settings => {
                self.page = Page::Settings;
                self.settings_field = 0;
                self.edit = None;
                self.settings_scroll = Kinetic::default();
            }
            To::Section(field) => {
                self.settings_field = field;
                self.edit = None;
                self.settings_scroll = Kinetic::default();
            }
            To::Field(field) => {
                if self.sections.section(field).0 != self.sections.section(self.settings_field).0 {
                    self.settings_scroll = Kinetic::default();
                }
                self.settings_field = field;
            }
            To::Edit {
                field,
                wheel_origin,
                from_setup,
            } => {
                self.page = Page::Settings;
                self.settings_field = field;
                self.edit = Some(Edit { wheel_origin });
                self.setup_return |= from_setup;
            }
            To::EditDone => self.edit = None,
            To::Setup(page) => {
                self.setup_return = false;
                self.page = page;
            }
            To::Sensors { from_setup } => {
                self.sensors_return |= from_setup;
                self.page = Page::Sensors;
            }
            To::LeaveSensors(page) => {
                self.sensors_return = false;
                self.page = page;
            }
            To::Ride => {
                self.page = Page::Ride;
                self.ride_page = self.first_page;
                self.climb = Follow::Idle;
                self.pause_hidden = false;
            }
            To::Summary { news } => {
                self.best_news = news;
                self.summary_page = 0;
                self.page = Page::Summary;
            }
            To::Home => {
                self.leave_summary();
                self.ride_page = RidePage::Main;
                self.page = Page::Home;
            }
            To::PastRide => {
                self.summary_page = 0;
                self.page = Page::PastRide;
            }
            To::Phone => {
                self.page = Page::Phone;
                self.phone_step = 0;
                self.phone_advanced = false;
            }
            To::PhoneCode => self.phone_step ^= 1,
            To::PhoneJoined => {
                self.phone_advanced = true;
                self.phone_step = 1;
            }
            To::RidePage(page) => self.ride_page_chosen(page),
            To::LeavePause { wrapped_to } => {
                self.pause_hidden = true;
                if core::mem::take(&mut self.pause_wrapped) {
                    self.ride_page_chosen(wrapped_to);
                }
            }
            To::PauseWrapped => {
                self.pause_hidden = false;
                self.pause_wrapped = true;
            }
            To::Road { road, page_on } => {
                let before = core::mem::replace(&mut self.road, road);
                let seen = climb_page::Seen {
                    before,
                    road,
                    riding: self.page == Page::Ride,
                    page_on,
                    ride_page: self.ride_page,
                };
                self.follow(climb_page::on_road(self.climb, seen));
            }
            To::WorkoutDone => {
                if self.ride_page == RidePage::Workout {
                    self.ride_page_chosen(RidePage::Main);
                }
                self.climb = self.climb.workout_done();
            }
            To::RideStarted => self.climb = Follow::Idle,
            To::Restore(restore) => {
                (self.ride_page, self.climb) = restore.ride;
                Pause {
                    hidden: self.pause_hidden,
                    wrapped: self.pause_wrapped,
                } = restore.pause;
                self.summary_page = restore.summary_page;
            }
            To::SummaryPage(page) => self.summary_page = page,
        }
        if self.page != Page::Settings {
            self.edit = None;
        }
        if self.page != from {
            why!("page", from, to = self.page);
        }
        if self.ride_page != ride_from {
            why!("ride_page", from = ride_from, to = self.ride_page);
        }
    }

    fn ride_page_chosen(&mut self, page: RidePage) {
        self.ride_page = page;
        self.follow(climb_page::on_paged(self.climb));
    }

    fn follow(&mut self, step: climb_page::Step) {
        self.climb = step.follow;
        if let Some(page) = step.show {
            self.ride_page = page;
        }
    }

    pub(super) fn page(&self) -> Page {
        self.page
    }

    pub(super) fn ride_page(&self) -> RidePage {
        self.ride_page
    }

    pub(super) fn settings_field(&self) -> usize {
        self.settings_field
    }

    pub(super) fn view<'a>(&self, local: Option<crate::clock::Local>) -> View<'a> {
        let cursor = match self.page {
            Page::Settings => self.settings_field,
            _ => 0,
        };
        let mut view = View::new(self.page, cursor, local);
        view.sections = self.sections;
        view.editing = self.page == Page::Settings && self.editing();
        view
    }

    pub(super) fn setting(&self) -> settings::Row {
        self.sections.row(self.settings_field)
    }

    pub(super) fn sections(&self) -> settings::Sections {
        self.sections
    }

    pub(super) fn set_sections(&mut self, sections: settings::Sections) {
        self.sections = sections;
    }

    pub(super) fn edit(&self) -> Option<Edit> {
        self.edit
    }

    pub(super) fn editing(&self) -> bool {
        self.edit.is_some()
    }

    pub(super) fn setup_return(&self) -> bool {
        self.setup_return
    }

    pub(super) fn sensors_return(&self) -> bool {
        self.sensors_return
    }

    pub(super) fn phone_step(&self) -> u8 {
        self.phone_step
    }

    pub(super) fn phone_advanced(&self) -> bool {
        self.phone_advanced
    }

    pub(super) fn climb(&self) -> Follow {
        self.climb
    }

    pub(super) fn pause_hidden(&self) -> bool {
        self.pause_hidden
    }

    pub(super) fn pause_wrapped(&self) -> bool {
        self.pause_wrapped
    }

    pub(super) fn paused_since(&self) -> Option<u64> {
        self.paused_since
    }

    pub(super) fn summary_page(&self) -> usize {
        self.summary_page
    }

    pub(super) fn first_frame(&self) -> Option<u64> {
        self.first_frame
    }

    pub(super) fn touches(&self) -> &Touches {
        &self.touches
    }

    pub(super) fn touches_mut(&mut self) -> &mut Touches {
        &mut self.touches
    }

    pub(super) fn settings_scroll(&self) -> &Kinetic {
        &self.settings_scroll
    }

    pub(super) fn settings_scroll_mut(&mut self) -> &mut Kinetic {
        &mut self.settings_scroll
    }

    pub(super) fn toast_shown(&self) -> Option<(Toast, u64)> {
        self.toast
    }

    pub(super) fn last_press(&self) -> u64 {
        self.last_press
    }

    pub(super) fn pressed(&mut self, received: u64) {
        self.last_press = received;
    }

    pub(super) fn reported(&mut self, role: Role, received: u64, hold_gap_ms: u64) -> Report {
        let index = match role {
            Role::Act => 0,
            Role::Back => 1,
            Role::Move => 2,
        };
        let last = &mut self.last_report[index];
        let held = last.is_some_and(|at| received.saturating_sub(at) < hold_gap_ms);
        *last = Some(received);
        if held { Report::Held } else { Report::Pressed }
    }

    pub(super) fn first_frame_at(&mut self, now: u64) -> u64 {
        *self.first_frame.get_or_insert(now)
    }

    pub(super) fn set_first_page(&mut self, page: RidePage) {
        self.first_page = page;
    }

    /// A return to setup lasts only while its editor or sensor menu does.
    pub(super) fn lapse_returns(&mut self) {
        if !(self.page == Page::Settings && self.editing()) {
            self.setup_return = false;
        }
        if self.page != Page::Sensors {
            self.sensors_return = false;
        }
    }

    pub(super) fn expire_toast(&mut self, now: u64) {
        if self.toast.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.toast = None;
        }
    }

    pub(super) fn chime(&mut self, cue: Cue) {
        self.chime = Some(cue);
    }

    pub(super) fn take_chime(&mut self) -> Option<Cue> {
        self.chime.take()
    }

    /// Show `message` over the content for a moment, replacing any other.
    pub(super) fn toast(&mut self, message: &str, tone: Tone, now: u64) {
        self.toast = Some((Toast::new(message, tone), now.saturating_add(TOAST_MS)));
    }

    /// Show `toast` for `ms`, replacing any other.
    pub(super) fn show(&mut self, toast: Toast, ms: u64, now: u64) {
        self.toast = Some((toast, now.saturating_add(ms)));
    }

    /// Say what the ride's status became; a ride the phone has not
    /// confirmed (`unsynced`) is never called saved.
    pub(super) fn announce(
        &mut self,
        status: Status,
        pages_over_pause: bool,
        unsynced: bool,
        now: u64,
    ) {
        let before = core::mem::replace(&mut self.announced, status);
        if before == status {
            return;
        }
        let riding = matches!(before, Status::Recording | Status::Paused);
        let (message, tone) = match (before, status) {
            (Status::Paused, Status::Recording) => {
                self.paused_since = None;
                self.chime = Some(Cue::Resume);
                ("RESUMED", Tone::Good)
            }
            (_, Status::Recording) => {
                self.paused_since = None;
                self.best_news = None;
                self.chime = Some(Cue::Start);
                ("RECORDING", Tone::Good)
            }
            (_, Status::Paused) => {
                self.paused_since = Some(now);
                self.pause_hidden = pages_over_pause;
                self.pause_wrapped = false;
                self.chime = Some(Cue::Pause);
                return;
            }
            (_, Status::Saved) if riding && self.page == Page::Summary => {
                // New bests take the saved chime's place, with a toast.
                let news = self.best_news.take();
                self.chime = Some(if news.is_some() {
                    Cue::Best
                } else {
                    Cue::Saved
                });
                if let Some(news) = news {
                    self.toast(news.as_str(), Tone::Good, now);
                    self.best_shown = Some(news);
                }
                return;
            }
            (_, Status::Saved) if riding && unsynced => ("SYNC TO SAVE", Tone::Warn),
            (_, Status::Saved) if riding => ("RIDE SAVED", Tone::Good),
            (_, Status::Recovered) => ("RIDE RECOVERED", Tone::Info),
            _ => return,
        };
        self.toast(message, tone, now);
    }

    /// A discarded ride was saved, then hidden or not: `message` instead of
    /// RIDE SAVED and its chime.
    pub(super) fn discarded(&mut self, message: &str, tone: Tone, now: u64) {
        self.announced = Status::Saved;
        self.toast(message, tone, now);
    }

    /// The toast of new bests belongs to the summary: leaving it takes that
    /// toast away, and only that one.
    fn leave_summary(&mut self) {
        let Some(shown) = self.best_shown.take() else {
            return;
        };
        if self
            .toast
            .as_ref()
            .is_some_and(|(toast, _)| toast.text() == Toast::new(shown.as_str(), Tone::Good).text())
        {
            self.toast = None;
        }
    }

    #[cfg(test)]
    pub(super) fn clear_toast(&mut self) {
        self.toast = None;
    }

    #[cfg(test)]
    pub(super) fn edit_off_settings(&mut self) {
        self.page = Page::Home;
        self.edit = Some(Edit {
            wheel_origin: Wheel::default(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_climb_over_a_finished_workout_goes_back_to_the_main_page() {
        let mut navigation = Navigation {
            climb: Follow::Shown {
                back_to: RidePage::Workout,
            },
            ..Navigation::default()
        };
        navigation.go(To::WorkoutDone);
        assert_eq!(
            navigation.climb,
            Follow::Shown {
                back_to: RidePage::Main
            }
        );
        navigation.climb = Follow::Shown {
            back_to: RidePage::Map,
        };
        navigation.go(To::WorkoutDone);
        assert_eq!(
            navigation.climb,
            Follow::Shown {
                back_to: RidePage::Map
            }
        );
    }

    #[test]
    fn only_settings_keeps_an_edit() {
        let edit = To::Edit {
            field: 0,
            wheel_origin: Wheel::default(),
            from_setup: true,
        };
        let mut navigation = Navigation::default();
        navigation.go(edit);
        navigation.go(To::Page(Page::Settings));
        assert!(navigation.editing());
        navigation.go(To::Setup(Page::Setup));
        assert!(!navigation.editing(), "setup's way back ends the edit");
        assert!(!navigation.setup_return);
    }
}
