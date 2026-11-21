//! First-run setup: which page is due, what its cards do, and how it hands
//! off to the Settings editor and the sensor menu and back.
//!
//! The step is stored with VANA's options, so setup resumes where the rider
//! left it. A record from before setup existed counts as set up.
use super::Runtime;
use super::navigation::To;
use crate::screens::settings::Row;
use crate::screens::setup::{Choice, Step};
use crate::screens::workout::{Page, View};
use device_api::ant::Ant;

impl Runtime {
    /// The setup page due, if setup is not done.
    pub(super) fn setup_step(&self) -> Option<Step> {
        let step = Step::stored(self.preferences.options.setup)?;
        // Firmware from before setup stored a fresh device's options as at
        // Welcome too; a rider who has since saved a sensor or a threshold
        // has set up already.
        let preferences = &self.preferences;
        let used = preferences.sensors.peers().count() > 0
            || preferences.profile != crate::ride::zones::TrainingProfile::default();
        (step != Step::Welcome || !used).then_some(step)
    }

    pub(super) fn first_page(&self) -> Page {
        use super::format::Format;
        if matches!(
            self.format,
            Format::Offered { .. } | Format::Resuming { .. } | Format::Running { .. }
        ) {
            Page::Format
        } else if self.setup_step().is_some() {
            Page::Setup
        } else {
            Page::Home
        }
    }

    /// Store that setup is at `step`, or done.
    fn set_setup(&mut self, step: Option<Step>, now: u64) {
        let mut options = self.preferences.options;
        options.setup = step.map_or(0, Step::byte);
        self.set_options(options, now);
        // Leaving setup is a deliberate choice: keep it at once.
        if step.is_none() {
            self.preferences.settle_now(now);
        }
        self.composed_focus(crate::compose::input::Focus::default());
    }

    /// Show `step`, or Home once setup is done.
    fn go_to_setup(&mut self, step: Option<Step>, now: u64) {
        self.set_setup(step, now);
        self.navigation.go(To::Setup(if step.is_some() {
            Page::Setup
        } else {
            Page::Home
        }));
    }

    /// Setup card `card` of the step due was taken, by ACT or a tap.
    pub(super) fn setup_card(&mut self, card: usize, now: u64, ant: &mut impl Ant) {
        let Some(step) = self.setup_step() else {
            return;
        };
        self.setup_choice(step, card, now, ant);
    }

    fn setup_choice(&mut self, step: Step, card: usize, now: u64, ant: &mut impl Ant) {
        let Some(&(_, choice)) = step.cards().get(card) else {
            return;
        };
        match choice {
            Choice::Next | Choice::Default => self.go_to_setup(step.next(), now),
            Choice::Edit => {
                let row = if step == Step::Power {
                    Row::Ftp
                } else {
                    Row::Threshold
                };
                let Some(field) = self.navigation.sections().field(row) else {
                    return;
                };
                self.navigation.go(To::Edit {
                    field,
                    wheel_origin: self.preferences.options.wheel,
                    from_setup: true,
                });
            }
            Choice::Pair => {
                // The sensor menu's way out returns to the last page.
                self.set_setup(Some(Step::Done), now);
                self.open_sensors(true, ant, now);
            }
            Choice::Ride => {
                self.set_setup(None, now);
                self.navigation.go(To::Page(Page::Ready));
            }
            Choice::Skip | Choice::Home => self.go_to_setup(None, now),
        }
    }

    /// The Settings editor finished; from setup, go on to the next page.
    pub(super) fn setup_edit_finished(&mut self, now: u64) -> bool {
        if !self.navigation.setup_return() {
            return false;
        }
        let step = self.setup_step().and_then(Step::next);
        self.go_to_setup(step, now);
        true
    }

    /// Where the sensor menu's way out leads: back to setup when it was
    /// opened from there.
    pub(super) fn after_sensors(&mut self) {
        let page = if self.navigation.sensors_return() && self.setup_step() == Some(Step::Done) {
            Page::Setup
        } else {
            Page::Home
        };
        self.navigation.go(To::LeaveSensors(page));
    }

    /// Notes beside the cards: the value a number has now.
    pub(super) fn setup_lines(&self, view: &mut View) {
        let step = self.setup_step();
        let lines = view.show_setup(step.unwrap_or(Step::Welcome));
        let Some(step) = step else {
            return;
        };
        let sensors = self.preferences.sensors.peers().count();
        lines[0] = crate::screens::setup::note(step, self.preferences.profile, sensors);
    }
}
