//! What a rider does on VANA, as button sequences that read the screen.
use crate::sim::{Sim, seconds};
use device_api::input::Button;

impl Sim {
    /// Choose a Home tile; the selected one is drawn black on cyan.
    #[track_caller]
    pub fn home(&mut self, label: &str) {
        for _ in 0..4 {
            if self.screen().run(label).is_some_and(|run| run.color == 0) {
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
            if screen.run(kind).is_some() && screen.run("CONNECT").is_some() {
                self.press(Button::BottomRight);
                self.wait_for("LIVE DATA", seconds(15)).unwrap();
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!(
            "no {kind} tile to connect; screen:\n{}",
            self.screen().text()
        );
    }

    /// Pair sensors from the Sensors page, then return Home.
    pub fn pair(&mut self, kinds: &[&str]) {
        self.home("SENSORS");
        self.wait_for("SCAN DONE", seconds(20)).unwrap();
        for kind in kinds {
            self.connect_sensor(kind);
        }
        for _ in 0..3 {
            if self.screen().contains("READY TO RIDE") {
                return;
            }
            self.press(Button::TopLeft);
        }
        panic!(
            "Sensors did not return Home; screen:\n{}",
            self.screen().text()
        );
    }

    /// Train from Home, through the connection check, onto a recording ride
    /// page. BottomRight acts: it starts, pauses and resumes a ride.
    pub fn start_ride(&mut self) {
        self.home("TRAIN");
        self.expect_text("CONNECTION CHECK");
        self.wait_for("ACTIVE TIME", seconds(5)).unwrap();
        self.press(Button::BottomRight);
        self.wait_for("RECORDING", seconds(2)).unwrap();
    }

    pub fn pause_ride(&mut self) {
        self.press(Button::BottomRight);
        self.wait_for("PAUSED", seconds(2)).unwrap();
    }

    pub fn resume_ride(&mut self) {
        self.press(Button::BottomRight);
        self.wait_for("RECORDING", seconds(2)).unwrap();
    }

    /// Pause if still recording (only a paused ride ends), then press TopLeft
    /// twice, confirming on "PRESS AGAIN TO SAVE", and wait for the save.
    pub fn stop_ride(&mut self) {
        if self.screen().run("RECORDING").is_some() {
            self.pause_ride();
        }
        self.press(Button::TopLeft);
        self.expect_text("PRESS AGAIN TO SAVE");
        self.press(Button::TopLeft);
        self.wait_for("SAVED", seconds(5)).unwrap();
    }

    /// TopLeft on a recording ride: close the lap and see its toast, such as
    /// "LAP 1 05:00 200W".
    #[track_caller]
    pub fn lap(&mut self, number: u16) -> String {
        assert!(
            self.screen().run("RECORDING").is_some(),
            "TopLeft laps only a recording ride; screen:\n{}",
            self.screen().text()
        );
        self.press(Button::TopLeft);
        let prefix = format!("LAP {number} ");
        self.screen()
            .find(&prefix)
            .into_iter()
            .find(|run| run.text.starts_with(&prefix))
            .unwrap_or_else(|| panic!("no {prefix:?} toast; screen:\n{}", self.screen().text()))
            .text
    }

    /// BottomLeft until the header shows a ride page indicator, such as "2/3".
    #[track_caller]
    pub fn ride_page(&mut self, indicator: &str) {
        for _ in 0..4 {
            if self.screen().contains(indicator) {
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
