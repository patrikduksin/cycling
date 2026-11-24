//! What a rider does on VANA, as button sequences that read the screen.
use crate::sim::{NoteKind, Sim, seconds};
use device_api::input::Role;

/// The value after `name=` in a status line such as `RIDE STATUS`'s.
pub fn status_field<'a>(status: &'a str, name: &str) -> Option<&'a str> {
    status
        .split_whitespace()
        .find_map(|part| part.strip_prefix(name)?.strip_prefix('='))
}

const WIDGET_RING: (usize, usize) = (40, 109);

impl Sim {
    /// The host reads its last outcome (`RIDE STATUS`) before VANA accepts another ride action.
    pub fn command_after_reading_outcome(&mut self, text: &str) -> (&'static str, String) {
        self.command("RIDE STATUS");
        self.command(text)
    }

    /// Run until the device is on, lit and past the boot page, or until
    /// `within_ms` have gone by; whether it got there.
    pub fn leave_boot_page(&mut self, within_ms: u64) -> bool {
        let from = self.now();
        loop {
            let booting = self.is_off() || !self.device.lit() || self.vana_page() == "boot";
            if !booting {
                return true;
            }
            if self.now() - from > within_ms {
                return false;
            }
            self.advance(10);
        }
    }

    /// Leave Home for a page. SENSORS is Home's status widget, and RIDE
    /// starts a ride.
    #[track_caller]
    pub fn home(&mut self, label: &str) {
        match label {
            "SENSORS" => return self.home_sensors(),
            "RIDE" => return self.press_home_start(),
            _ => {}
        }
        for _ in 0..crate::sim::HOME_STOPS + 2 {
            if self.screen().white(label) {
                self.press(Role::Act);
                return;
            }
            self.press(Role::Move);
        }
        panic!("no Home tile {label:?}; screen:\n{}", self.screen().text());
    }

    /// On SENSORS, MOVE to the ADD SENSOR row and open NEARBY, the page
    /// that searches. Nothing to do when it shows already.
    #[track_caller]
    pub fn nearby_page(&mut self) {
        for _ in 0..14 {
            if on_nearby(&self.screen()) {
                return;
            }
            let screen = self.screen();
            let row = screen.run("ADD SENSOR");
            let act = row.is_some_and(|row| focused(&screen, &row));
            self.press(if act { Role::Act } else { Role::Move });
        }
        panic!("NEARBY does not open; screen:\n{}", self.screen().text());
    }

    #[track_caller]
    pub fn connect_sensor(&mut self, kind: &str) {
        self.nearby_page();
        self.focus_row(kind);
        self.press(Role::Act);
        self.expect_text(" ADDED");
    }

    /// On SENSORS, MOVE to a saved sensor and open its detail page.
    #[track_caller]
    pub fn open_sensor_detail(&mut self, kind: &str) {
        if on_nearby(&self.screen()) {
            self.press(Role::Back);
        }
        self.focus_row(kind);
        self.press(Role::Act);
        self.expect_text("ANT+");
    }

    #[track_caller]
    pub fn disconnect_sensor(&mut self, kind: &str) {
        self.open_sensor_detail(kind);
        self.advance(vana::screens::sensors::SETTLE_MS);
        self.press(Role::Move);
        self.expect_text("FORGET");
        self.advance(vana::screens::sensors::SETTLE_MS);
        self.press(Role::Act);
    }

    /// Wait until SENSORS shows `kind`'s row LIVE.
    #[track_caller]
    fn wait_live(&mut self, kind: &str) {
        for _ in 0..150 {
            if row_live(&self.screen(), kind) {
                return;
            }
            self.advance(100);
        }
        panic!("{kind} is not LIVE; screen:\n{}", self.screen().text());
    }

    /// MOVE until the row naming `kind` has the focus ring.
    #[track_caller]
    fn focus_row(&mut self, kind: &str) {
        for _ in 0..14 {
            let screen = self.screen();
            let mut rows = screen.frame().read().into_iter();
            if rows.any(|run| run.text == row_name(kind) && focused(&screen, &run)) {
                return;
            }
            self.press(Role::Move);
        }
        panic!("no {kind} row to focus; screen:\n{}", self.screen().text());
    }

    /// Pair sensors from one search on NEARBY, see them LIVE on SENSORS,
    /// then return Home.
    pub fn pair(&mut self, kinds: &[&str]) {
        self.home("SENSORS");
        self.nearby_page();
        self.wait_for("SCAN DONE", seconds(20)).unwrap();
        for kind in kinds {
            self.connect_sensor(kind);
        }
        self.press(Role::Back);
        for kind in kinds {
            self.wait_live(kind);
        }
        for _ in 0..3 {
            if self.on_home() {
                return;
            }
            self.press(Role::Back);
        }
        panic!(
            "Sensors did not return Home; screen:\n{}",
            self.screen().text()
        );
    }

    pub fn on_home(&self) -> bool {
        self.device.lit() && self.vana_page() == "home"
    }

    fn widget_focused(&self) -> bool {
        use vana::screens::theme::{DARK, LIGHT};
        let screen = self.screen();
        let at = vana::screens::scaled::Scaled::fitting(screen.width, screen.height);
        let unit = |n: usize| at.map_or(n, |at| at.unit(n));
        let (x, y) = (unit(WIDGET_RING.0), unit(WIDGET_RING.1));
        let pixel = screen.pixels[y * screen.width + x];
        pixel == DARK.focus || pixel == LIGHT.focus
    }

    #[track_caller]
    pub fn home_sensors(&mut self) {
        for _ in 0..crate::sim::HOME_STOPS + 2 {
            if self.on_home() && self.widget_focused() {
                self.press(Role::Act);
                return;
            }
            self.press(Role::Move);
        }
        panic!(
            "Home's widget never takes the focus; screen:\n{}",
            self.screen().text()
        );
    }

    /// Press Home's start, the stop Home opens on. A second press covers a
    /// dimmed screen, which a press only wakes. Nothing to press when a
    /// rider rolling off on Home started the ride already.
    pub fn press_home_start(&mut self) {
        if self.vana_page() == "ride" {
            return;
        }
        assert!(self.on_home(), "{}", self.screen().text());
        self.press(Role::Act);
        if self.on_home() {
            self.press(Role::Act);
        }
    }

    pub fn start_ride(&mut self) {
        if self.vana_page() == "ride" {
            return;
        }
        self.press_home_start();
        self.wait_for("RECORDING", seconds(2)).unwrap();
    }

    /// The page VANA shows, such as "ride_done".
    pub fn vana_page(&self) -> &'static str {
        self.device.sdk.page().name()
    }

    pub fn pause_ride(&mut self) {
        self.press(Role::Act);
        self.wait_for("Paused", seconds(3)).unwrap();
    }

    /// From the paused screen, swipe to the ride page shown before the pause.
    #[track_caller]
    pub fn show_ride_pages(&mut self) {
        if self.screen().contains("Ride time") {
            self.swipe_next();
        }
        assert!(
            !self.screen().contains("Ride time"),
            "{}",
            self.screen().text()
        );
    }

    pub fn resume_ride(&mut self) {
        self.press(Role::Act);
        self.wait_for("RESUMED", seconds(3)).unwrap();
    }

    /// Set an option from the terminal, as `OPTION AUTO_LAP START` does.
    #[track_caller]
    pub fn option(&mut self, name: &str, value: &str) {
        let (status, out) = self.command(&format!("OPTION {name} {value}"));
        assert_eq!(status, "OK", "OPTION {name} {value}: {out}");
    }

    /// Set a profile field from the terminal, as `PROFILE FTP 250` does.
    #[track_caller]
    pub fn profile(&mut self, field: &str, value: &str) {
        let (status, out) = self.command(&format!("PROFILE {field} {value}"));
        assert_eq!(status, "OK", "PROFILE {field} {value}: {out}");
    }

    /// Set the training profile from the terminal.
    #[track_caller]
    pub fn set_profile(&mut self, ftp_watts: u16, lthr_bpm: u16) {
        self.profile("FTP", &ftp_watts.to_string());
        self.profile("LTHR", &lthr_bpm.to_string());
    }

    /// The recorder state from `RIDE STATUS`, such as "recording"; the
    /// status row can be showing a toast instead.
    pub fn ride_state(&mut self) -> String {
        let (_, status) = self.command("RIDE STATUS");
        status_field(&status, "state")
            .unwrap_or_default()
            .to_string()
    }

    /// Choose the workout `name` from the terminal; the next ride plays it.
    #[track_caller]
    pub fn choose_workout(&mut self, name: &str) {
        let (status, list) = self.command("WORKOUT LIST");
        assert_eq!(status, "OK", "WORKOUT LIST: {list}");
        let rows: usize = status_field(&list, "rows")
            .and_then(|rows| rows.parse().ok())
            .unwrap_or_else(|| panic!("WORKOUT LIST: {list}"));
        let wanted = format!("workout={}", name.replace(' ', "_"));
        for row in 1..rows {
            let (status, out) = self.command(&format!("WORKOUT SELECT {row}"));
            assert_eq!(status, "OK", "WORKOUT SELECT {row}: {out}");
            let (_, chosen) = self.command("WORKOUT STATUS");
            if chosen.split_whitespace().any(|field| field == wanted) {
                return;
            }
        }
        panic!("no workout {name:?}: {list}");
    }

    /// From Home, choose the workout `name` and start its ride.
    #[track_caller]
    pub fn start_workout(&mut self, name: &str) {
        self.choose_workout(name);
        self.start_ride();
    }

    /// `WORKOUT STATUS` as fields, such as `step` = "3/12".
    pub fn workout(&mut self) -> std::collections::HashMap<String, String> {
        let (status, out) = self.command("WORKOUT STATUS");
        assert_eq!(status, "OK", "WORKOUT STATUS: {out}");
        out.split_whitespace()
            .filter_map(|part| part.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Pause if still recording (only a paused ride ends), then BACK asks
    /// "End ride?" and ACT answers it; Ride done opens at once, and this
    /// waits for the ride's last samples to be written. Ends on Ride done.
    pub fn stop_ride(&mut self) {
        if self.ride_state() == "recording" {
            self.pause_ride();
        }
        self.press(Role::Back);
        self.expect_text("End ride?");
        self.press(Role::Act);
        self.wait_until(seconds(5), |screen| {
            screen.contains("saved") || screen.contains("Sync to save")
        })
        .unwrap();
        for _ in 0..50 {
            let (_, status) = self.command("RIDE STATUS");
            let state = status_field(&status, "state").unwrap_or_default();
            if state != "paused" && status_field(&status, "pending") == Some("0") {
                return;
            }
            self.advance(100);
        }
        panic!("the ride never finished saving");
    }

    /// Leave Ride done with ACT and land on Home.
    #[track_caller]
    pub fn done(&mut self) {
        self.press(Role::Act);
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
        self.press(Role::Back);
        let prefix = format!("LAP {number} ");
        self.screen()
            .find(&prefix)
            .into_iter()
            // The toast names the lap's time; the lap page's captions do not.
            .find(|run| run.text.starts_with(&prefix) && run.text.contains(':'))
            .unwrap_or_else(|| panic!("no {prefix:?} toast; screen:\n{}", self.screen().text()))
            .text
    }

    /// The page the trace last saw drawn, such as "ride/map".
    pub fn ride_page_name(&self) -> Option<String> {
        self.seen().page.map(|(_, page)| page)
    }

    /// BottomLeft until the trace sees the ride page `name`, such as
    /// "ride/map". On the paused screen BottomLeft asks End ride?, so a
    /// swipe moves on from there.
    #[track_caller]
    pub fn ride_page(&mut self, name: &str) {
        for _ in 0..8 {
            match self.ride_page_name() {
                Some(page) if page == name => return,
                Some(page) if page == "ride/paused" => self.swipe_next(),
                _ => self.press(Role::Move),
            }
        }
        panic!(
            "no ride page {name}, on {:?}; screen:\n{}",
            self.ride_page_name(),
            self.screen().text()
        );
    }

    /// Run until the panel shows its next frame, two seconds at most. A
    /// page the ride brings up by itself is in the trace a few passes
    /// before its first frame is on the panel.
    pub fn next_frame(&mut self) {
        let shown = self.device.display.0.borrow().submissions;
        for _ in 0..200 {
            self.advance(10);
            if self.device.display.0.borrow().submissions != shown {
                return;
            }
        }
    }

    #[track_caller]
    pub fn connect_phone(&mut self) {
        self.need(crate::sim::Need::Wifi);
        let (status, reply) = self.command("PHONE START");
        assert_eq!(status, "ACCEPTED", "PHONE START: {reply}");
        self.advance(seconds(1));
        self.device.sim_mut().access_point.clients = 1;
        self.advance(seconds(1));
    }

    #[track_caller]
    pub fn disconnect_phone(&mut self) {
        let (status, reply) = self.command("PHONE STOP");
        assert_eq!(status, "ACCEPTED", "PHONE STOP: {reply}");
        self.advance(seconds(1));
    }

    /// A request from the phone that joined, with the session's token as its
    /// page sends it, answered by the device: the status and the body.
    #[track_caller]
    pub fn phone_request(
        &mut self,
        method: device_api::server::Method,
        path: &str,
        body: &[u8],
    ) -> (u16, String) {
        self.need(crate::sim::Need::Wifi);
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
            .sim_mut()
            .phone_server
            .ask(method, path, token.text(), body);
        for _ in 0..600 {
            if let Some(response) = self.device.sim_mut().phone_server.response() {
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

/// NEARBY names itself in its title.
fn on_nearby(screen: &crate::sim::Screen) -> bool {
    screen.run("NEARBY").is_some()
}

/// The row name, in the capitals a list draws, of the sensor whose page
/// title is `kind` in capitals, such as "POWER" for "POWER METER".
fn row_name(kind: &str) -> String {
    let names = vana::screens::sensors::NAMES.iter();
    let named = names
        .into_iter()
        .find(|names| names.long.eq_ignore_ascii_case(kind));
    named.map_or(kind, |names| names.short).to_ascii_uppercase()
}

/// The state line under `kind`'s name reads LIVE.
fn row_live(screen: &crate::sim::Screen, kind: &str) -> bool {
    let runs = screen.frame().read();
    let Some(name) = runs.iter().find(|run| run.text == row_name(kind)) else {
        return false;
    };
    runs.iter().any(|run| {
        run.text.contains("LIVE")
            && run.x.abs_diff(name.x) < 4
            && (name.y + 8..name.y + 30).contains(&run.y)
    })
}

/// A sensor row has the focus: the accent ring runs down its card's left
/// edge, in the margin left of the card's icon.
fn focused(screen: &crate::sim::Screen, run: &crate::screen::TextRun) -> bool {
    let accent = vana::screens::chrome::ACCENT;
    let accent = screen.look.ink(accent).0;
    let row = (run.y + 8) * screen.width;
    let margin = screen.pixels.get(row..row + screen.width / 12);
    margin.is_some_and(|margin| margin.contains(&accent))
}
