use super::cues::Cue;
use super::presentation::TOAST_MS;
use crate::ride::log::Status;
use crate::screens::chrome::{Toast, Tone};
use crate::screens::ride::RidePage;
use crate::screens::touch::Touches;
use crate::screens::workout::{Page, View};
use device_api::input::Role;
use firmware_services::why;

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
    pub(super) ride: RidePage,
    pub(super) pause: Pause,
}

pub(super) enum To {
    Page(Page),
    Ride,
    Home,
    RidePage(RidePage),
    LeavePause { wrapped_to: RidePage },
    PauseWrapped,
    WorkoutDone,
    Restore(Restore),
}

pub(super) struct Navigation {
    page: Page,
    /// The ride page shown while `page` is the ride screen.
    ride_page: RidePage,
    last_press: u64,
    /// When VANA drew its first frame; the brand flash counts from here.
    first_frame: Option<u64>,
    /// Latest report of each button, to tell a held button from a new press.
    last_report: [Option<u64>; 3],
    /// The finger on the screen, if any, and what it is bound to.
    touches: Touches,
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
    /// A sound the next `alert` plays.
    chime: Option<Cue>,
}

state!(Navigation {
    page,
    ride_page,
    last_press,
    first_frame,
    last_report,
    touches,
    toast,
    announced,
    paused_since,
    pause_hidden,
    first_page,
    pause_wrapped,
    chime,
});

impl Default for Navigation {
    fn default() -> Self {
        Self {
            page: Page::Boot,
            ride_page: RidePage::Main,
            last_press: 0,
            first_frame: None,
            last_report: [None; 3],
            touches: Touches::default(),
            toast: None,
            announced: Status::Ready,
            paused_since: None,
            pause_hidden: false,
            first_page: RidePage::Main,
            pause_wrapped: false,
            chime: None,
        }
    }
}

impl Navigation {
    pub(super) fn go(&mut self, to: To) {
        let (from, ride_from) = (self.page, self.ride_page);
        match to {
            To::Page(page) => self.page = page,
            To::Ride => {
                self.page = Page::Ride;
                self.ride_page = self.first_page;
                self.pause_hidden = false;
            }
            To::Home => {
                self.ride_page = RidePage::Main;
                self.page = Page::Home;
            }
            To::RidePage(page) => self.ride_page = page,
            To::LeavePause { wrapped_to } => {
                self.pause_hidden = true;
                if core::mem::take(&mut self.pause_wrapped) {
                    self.ride_page = wrapped_to;
                }
            }
            To::PauseWrapped => {
                self.pause_hidden = false;
                self.pause_wrapped = true;
            }
            To::WorkoutDone => {
                if self.ride_page == RidePage::Workout {
                    self.ride_page = RidePage::Main;
                }
            }
            To::Restore(restore) => {
                self.ride_page = restore.ride;
                Pause {
                    hidden: self.pause_hidden,
                    wrapped: self.pause_wrapped,
                } = restore.pause;
            }
        }
        if self.page != from {
            why!("page", from, to = self.page);
        }
        if self.ride_page != ride_from {
            why!("ride_page", from = ride_from, to = self.ride_page);
        }
    }

    pub(super) fn page(&self) -> Page {
        self.page
    }

    pub(super) fn ride_page(&self) -> RidePage {
        self.ride_page
    }

    pub(super) fn view<'a>(&self, local: Option<crate::clock::Local>) -> View<'a> {
        View::new(self.page, 0, local)
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

    pub(super) fn first_frame(&self) -> Option<u64> {
        self.first_frame
    }

    pub(super) fn touches(&self) -> &Touches {
        &self.touches
    }

    pub(super) fn touches_mut(&mut self) -> &mut Touches {
        &mut self.touches
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
            (_, Status::Saved) if riding && self.page == Page::Done => {
                self.chime = Some(Cue::Saved);
                return;
            }
            (_, Status::Saved) if riding && unsynced => ("SYNC TO SAVE", Tone::Warn),
            (_, Status::Saved) if riding => {
                self.chime = Some(Cue::Saved);
                ("RIDE SAVED", Tone::Good)
            }
            (_, Status::Recovered) => ("RIDE RECOVERED", Tone::Info),
            _ => return,
        };
        self.toast(message, tone, now);
    }

    #[cfg(test)]
    pub(super) fn clear_toast(&mut self) {
        self.toast = None;
    }
}
