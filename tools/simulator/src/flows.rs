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

    /// Train from Home, through the connection check, onto a recording ride page.
    pub fn start_ride(&mut self) {
        self.home("TRAIN");
        self.expect_text("CONNECTION CHECK");
        self.wait_for("ACTIVE TIME", seconds(5)).unwrap();
        self.press(Button::BottomLeft);
        self.wait_for("RECORDING", seconds(2)).unwrap();
    }

    pub fn pause_ride(&mut self) {
        self.press(Button::BottomLeft);
        self.wait_for("PAUSED", seconds(2)).unwrap();
    }

    pub fn resume_ride(&mut self) {
        self.press(Button::BottomLeft);
        self.wait_for("RECORDING", seconds(2)).unwrap();
    }

    pub fn stop_ride(&mut self) {
        self.press(Button::BottomRight);
        self.expect_text("STOP TO SAVE");
        self.press(Button::BottomRight);
        self.wait_for("SAVED", seconds(5)).unwrap();
    }
}
