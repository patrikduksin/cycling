//! What a rider does on VANA, as button sequences that read the screen.
use crate::sim::{Sim, seconds};
use device_api::input::Button;

impl Sim {
    /// Choose a Home card, RIDE or SENSORS; the focused one has a white title.
    #[track_caller]
    pub fn home(&mut self, label: &str) {
        for _ in 0..4 {
            if self
                .screen()
                .run(label)
                .is_some_and(|run| run.color == 0xffff)
            {
                self.press(Button::BottomRight);
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!("no Home tile {label:?}; screen:\n{}", self.screen().text());
    }

    /// Step through Sensors tiles to a sensor kind and connect it.
    pub fn connect_sensor(&mut self, kind: &str) {
        for _ in 0..12 {
            let screen = self.screen();
            if screen.run(kind).is_some() && focused_nearby(&screen) {
                self.press(Button::BottomRight);
                self.wait_for("LIVE", seconds(15)).unwrap();
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!(
            "no {kind} tile to connect; screen:\n{}",
            self.screen().text()
        );
    }

    /// Step through Sensors tiles to a nearby sensor of an already saved
    /// kind and confirm it replaces the saved one.
    pub fn replace_sensor(&mut self, kind: &str) {
        for _ in 0..20 {
            let screen = self.screen();
            // A nearby sensor of a saved kind shows its number to tell them apart.
            if screen.run(kind).is_some() && screen.contains("SENSOR / ") {
                self.press(Button::BottomRight);
                self.expect_text("REPLACE SENSOR");
                self.press(Button::BottomRight);
                self.wait_for("LIVE", seconds(15)).unwrap();
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!(
            "no {kind} tile to replace; screen:\n{}",
            self.screen().text()
        );
    }

    /// Step through Sensors tiles to a saved sensor and disconnect it, which
    /// removes it from the saved sensors.
    pub fn disconnect_sensor(&mut self, kind: &str) {
        for _ in 0..20 {
            let screen = self.screen();
            if screen.run(kind).is_some() && !focused_nearby(&screen) {
                self.press(Button::BottomRight);
                self.expect_text("DISCONNECT");
                self.press(Button::BottomRight);
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!("no saved {kind} tile; screen:\n{}", self.screen().text());
    }

    /// Pair sensors from the Sensors page, then return Home.
    pub fn pair(&mut self, kinds: &[&str]) {
        self.home("SENSORS");
        self.wait_for("SCAN DONE", seconds(20)).unwrap();
        for kind in kinds {
            self.connect_sensor(kind);
        }
        for _ in 0..3 {
            if self.on_home() {
                return;
            }
            self.press(Button::TopLeft);
        }
        panic!(
            "Sensors did not return Home; screen:\n{}",
            self.screen().text()
        );
    }

    /// Home shows its big RIDE card.
    pub fn on_home(&self) -> bool {
        self.screen()
            .run("RIDE")
            .is_some_and(|run| run.smooth && run.scale >= 3)
    }

    /// Ride from Home, through the connection check, onto a recording ride
    /// page. BottomRight acts: it starts, pauses and resumes a ride.
    pub fn start_ride(&mut self) {
        self.home("RIDE");
        self.expect_text("CONNECTION CHECK");
        self.wait_for("KM/H", seconds(5)).unwrap();
        self.press(Button::BottomRight);
        self.wait_for("RECORDING", seconds(2)).unwrap();
    }

    pub fn pause_ride(&mut self) {
        self.press(Button::BottomRight);
        // A lap toast may cover the status row for up to two seconds.
        self.wait_for("PAUSED", seconds(3)).unwrap();
    }

    pub fn resume_ride(&mut self) {
        self.press(Button::BottomRight);
        self.wait_for("RESUMED", seconds(3)).unwrap();
    }

    /// From Home, open Settings and set the training profile with buttons:
    /// BottomLeft moves to a field, BottomRight edits it, BottomLeft/Right
    /// step it by five and TopLeft finishes. Ends on Settings.
    #[track_caller]
    pub fn edit_profile(&mut self, ftp_watts: u16, lthr_bpm: u16) {
        self.home("SETTINGS");
        for (index, target) in [ftp_watts, lthr_bpm].into_iter().enumerate() {
            if index > 0 {
                self.press(Button::BottomLeft);
            }
            self.press(Button::BottomRight);
            self.step_profile_field(index, target, |sim, raise| {
                sim.press(if raise {
                    Button::BottomRight
                } else {
                    Button::BottomLeft
                })
            });
            self.press(Button::TopLeft);
        }
    }

    /// From Home, open Settings and set the training profile by touch: tap a
    /// field, then the minus or plus pad. TopLeft finishes. Ends on Settings.
    #[track_caller]
    pub fn edit_profile_by_touch(&mut self, ftp_watts: u16, lthr_bpm: u16) {
        self.home("SETTINGS");
        for (index, target) in [ftp_watts, lthr_bpm].into_iter().enumerate() {
            self.tap(120, 86 + 78 * index as u16);
            self.step_profile_field(index, target, |sim, raise| {
                sim.tap(if raise { 176 } else { 64 }, 237)
            });
            self.press(Button::TopLeft);
        }
    }

    #[track_caller]
    fn step_profile_field(&mut self, index: usize, target: u16, step: impl Fn(&mut Self, bool)) {
        let label = ["FTP WATTS", "LTHR BPM"][index];
        for _ in 0..200 {
            let screen = self.screen();
            let value: u16 = screen
                .field(label)
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| panic!("no {label}; screen:\n{}", screen.text()));
            if value == target {
                break;
            }
            step(self, value < target);
        }
        self.expect_field(label, &target.to_string());
    }

    /// The recorder state from `RIDE STATUS`, such as "recording"; the
    /// status row can be showing a toast instead.
    pub fn ride_state(&mut self) -> String {
        let (_, status) = self.command("RIDE STATUS");
        status
            .split_whitespace()
            .find_map(|part| part.strip_prefix("state="))
            .unwrap_or_default()
            .to_string()
    }

    /// Pause if still recording (only a paused ride ends), then press TopLeft
    /// twice, confirming on "PRESS AGAIN TO SAVE", and wait for the save.
    pub fn stop_ride(&mut self) {
        if self.ride_state() == "recording" {
            self.pause_ride();
        }
        self.press(Button::TopLeft);
        self.expect_text("PRESS AGAIN TO SAVE");
        self.press(Button::TopLeft);
        self.wait_for("RIDE SAVED", seconds(5)).unwrap();
    }

    /// TopLeft on a recording ride: close the lap and see its toast, such as
    /// "LAP 1 05:00 200W".
    #[track_caller]
    pub fn lap(&mut self, number: u16) -> String {
        assert_eq!(
            self.ride_state(),
            "recording",
            "TopLeft laps only a recording ride; screen:\n{}",
            self.screen().text()
        );
        self.press(Button::TopLeft);
        let prefix = format!("LAP {number} ");
        self.screen()
            .find(&prefix)
            .into_iter()
            // The toast names the lap's time; the lap page's captions do not.
            .find(|run| run.text.starts_with(&prefix) && run.text.contains(':'))
            .unwrap_or_else(|| panic!("no {prefix:?} toast; screen:\n{}", self.screen().text()))
            .text
    }

    /// BottomLeft until the status bar's page dots read `indicator`, such as "2/3".
    #[cfg(feature = "cycling")]
    #[track_caller]
    pub fn ride_page(&mut self, indicator: &str) {
        for _ in 0..5 {
            if self.screen().page().as_deref() == Some(indicator) {
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!(
            "no ride page {indicator}; screen:\n{}",
            self.screen().text()
        );
    }
}

/// The Sensors status line names a nearby sensor, not a saved one.
fn focused_nearby(screen: &crate::sim::Screen) -> bool {
    [
        "READY TO CONNECT",
        "VEHICLES BEHIND",
        "SENSOR / ",
        "NOT SEEN",
        "GONE / ",
    ]
    .iter()
    .any(|status| screen.contains(status))
}
