//! What a rider does on VANA, as button sequences that read the screen.
use crate::sim::{NoteKind, Sim, seconds};
use device_api::input::Button;

impl Sim {
    /// Choose a Home card, such as RIDE, SENSORS, TRAIN or RIDES; the focused
    /// one has a white title. One extra press covers a dimmed screen, which a
    /// press only wakes.
    #[track_caller]
    pub fn home(&mut self, label: &str) {
        for _ in 0..crate::sim::HOME_CARDS + 2 {
            if self.screen().white(label) {
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

    /// Step through Sensors tiles to a saved sensor and open its detail page.
    pub fn open_sensor_detail(&mut self, kind: &str) {
        for _ in 0..20 {
            let screen = self.screen();
            if screen.run(kind).is_some() && !focused_nearby(&screen) {
                self.press(Button::BottomRight);
                self.expect_text("DISCONNECT");
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!("no saved {kind} tile; screen:\n{}", self.screen().text());
    }

    /// Step through Sensors tiles to a saved sensor and disconnect it, which
    /// removes it from the saved sensors.
    pub fn disconnect_sensor(&mut self, kind: &str) {
        self.open_sensor_detail(kind);
        self.press(Button::BottomRight);
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

    /// Ride from Home: RIDE opens Ready, and BottomRight starts recording
    /// onto the ride pages. BottomRight acts: it starts, pauses and resumes.
    pub fn start_ride(&mut self) {
        self.home("RIDE");
        self.expect_text("READY");
        self.press(Button::BottomRight);
        self.wait_for("RECORDING", seconds(2)).unwrap();
    }

    /// The page VANA shows, such as "summary".
    #[cfg(feature = "cycling")]
    pub fn vana_page(&self) -> &'static str {
        self.device.sdk.page().name()
    }

    pub fn pause_ride(&mut self) {
        self.press(Button::BottomRight);
        // A lap toast may cover the status row for up to two seconds.
        self.wait_for("PAUSED", seconds(3)).unwrap();
    }

    /// From the paused screen, MOVE to the ride page shown before the pause.
    #[track_caller]
    pub fn show_ride_pages(&mut self) {
        if self.screen().contains("RIDE TIME") {
            self.press(Button::BottomLeft);
        }
        assert!(
            !self.screen().contains("RIDE TIME"),
            "{}",
            self.screen().text()
        );
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

    /// On Settings, swipe to the section with the `label` row, then tap the
    /// row until it reads `value`. Ends on that section.
    #[track_caller]
    pub fn set_option(&mut self, label: &str, value: &str) {
        for _ in 0..16 {
            if self.screen().run(label).is_some() {
                break;
            }
            self.swipe_next();
        }
        const TAPS: usize = 12;
        for tap in 0..=TAPS {
            let screen = self.screen();
            if screen.field(label).as_deref() == Some(value) {
                self.note(NoteKind::Check, format!("{label} shows {value}"));
                return;
            }
            if tap == TAPS {
                break;
            }
            let run = screen
                .run(label)
                .unwrap_or_else(|| panic!("no {label} row; screen:\n{}", screen.text()));
            self.tap(run.x as u16 + 4, run.y as u16 + 4);
        }
        panic!(
            "{label} never read {value}; screen:\n{}",
            self.screen().text()
        );
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

    /// From Home, open TRAIN and choose the workout named `name` (or FREE
    /// RIDE): MOVE until its card is focused, then ACT. Ends on Ready, which
    /// names it.
    #[track_caller]
    pub fn choose_workout(&mut self, name: &str) {
        self.home("TRAIN");
        self.expect_text("WORKOUTS");
        for _ in 0..24 {
            if self.screen().white(name) {
                self.press(Button::BottomRight);
                assert_eq!(self.vana_page(), "ready");
                self.expect_text(name);
                return;
            }
            self.press(Button::BottomLeft);
        }
        panic!("no workout {name:?}; screen:\n{}", self.screen().text());
    }

    /// `WORKOUT STATUS` as fields, such as `step` = "3/12".
    #[cfg(feature = "cycling")]
    pub fn workout(&mut self) -> std::collections::HashMap<String, String> {
        let (status, out) = self.command("WORKOUT STATUS");
        assert_eq!(status, "OK", "WORKOUT STATUS: {out}");
        out.split_whitespace()
            .filter_map(|part| part.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Pause if still recording (only a paused ride ends), then TopLeft asks
    /// "End ride?" and BottomRight answers it; wait for RIDE SAVED on the
    /// summary. Ends on the summary.
    pub fn stop_ride(&mut self) {
        if self.ride_state() == "recording" {
            self.pause_ride();
        }
        self.press(Button::TopLeft);
        self.expect_text("END RIDE?");
        self.press(Button::BottomRight);
        self.wait_for("RIDE SAVED", seconds(5)).unwrap();
    }

    /// Finish the summary with BottomRight and land on Home.
    #[track_caller]
    pub fn done(&mut self) {
        self.press(Button::BottomRight);
        assert!(self.on_home(), "{}", self.screen().text());
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
        for _ in 0..8 {
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

    /// From Home, open Connect phone (Settings, PHONE) and let a phone join
    /// the access point, as the rider and their phone would.
    #[cfg(feature = "cycling")]
    #[track_caller]
    pub fn connect_phone(&mut self) {
        self.home("SETTINGS");
        for _ in 0..=vana::screens::settings::SECTIONS.len() {
            if self.screen().run("PHONE").is_some() {
                break;
            }
            self.swipe_next();
        }
        let row = self.screen().run("PHONE").expect("phone row in Settings");
        self.tap(row.x as u16 + 4, row.y as u16 + 4);
        self.advance(seconds(1));
        assert_eq!(self.vana_page(), "phone");
        self.device.access_point.clients = 1;
        self.advance(seconds(1));
    }

    /// A request from the phone that joined, with the session's token as its
    /// page sends it, answered by the device: the status and the body.
    #[cfg(feature = "cycling")]
    #[track_caller]
    pub fn phone_request(
        &mut self,
        method: device_api::server::Method,
        path: &str,
        body: &[u8],
    ) -> (u16, String) {
        let token = self
            .device
            .sdk
            .phone_status()
            .expect("a phone session")
            .token;
        self.note(
            NoteKind::Device,
            format!("phone {method:?} {path} ({} bytes)", body.len()),
        );
        self.device
            .phone_server
            .ask(method, path, token.text(), body);
        for _ in 0..600 {
            if let Some(response) = self.device.phone_server.response() {
                let text = String::from_utf8_lossy(response).into_owned();
                let status = text
                    .split(' ')
                    .nth(1)
                    .and_then(|code| code.parse().ok())
                    .unwrap_or_else(|| panic!("no status in {text:?}"));
                let body = text.split_once("\r\n\r\n").map_or("", |(_, body)| body);
                return (status, body.to_string());
            }
            self.advance(100);
        }
        panic!("the device never answered {method:?} {path}");
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
