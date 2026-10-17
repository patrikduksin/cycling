//! An interactive session: keys press buttons and change the world while
//! simulated time runs at a chosen multiple of real time. `sim-live` shows it.
use crate::ant::device_type;
use crate::artifacts::Canvas;
use crate::recording::{
    ACCENT, AMBER, BACKGROUND, BEZEL, DIM, FAIL, INK, MARGIN, PANEL_WIDTH, PASS, SCALE, clock, wrap,
};
use crate::route::{Point, Route};
use crate::sim::{HOLD_REPEAT_MS, NoteKind, Sim};
use crate::world::{Side, Vehicle};
use device_api::input::Button;

/// The firmware loop's pass length.
const STEP_MS: u64 = 10;
/// Real time a slow frame may owe; beyond it the clock slips rather than stalls.
const MAX_OWED_REAL_MS: f64 = 250.0;
/// A key down this long in real time is a held button, repeated by the companion.
const HOLD_AFTER_MS: f64 = 300.0;
pub const SPEEDS: [u32; 3] = [1, 10, 60];
const BATTERY: [u8; 4] = [80, 40, 15, 5];

/// Sensors near the rider: toggle key, label, ANT type and number.
const SENSORS: [(char, &str, u8, u16); 4] = [
    ('h', "HEART", device_type::HEART_RATE, 4242),
    ('p', "POWER", device_type::POWER, 1234),
    ('c', "SPEED/CAD", device_type::SPEED_CADENCE, 777),
    ('r', "RADAR", device_type::RADAR, 31),
];

pub const KEYS: [(&str, &str); 12] = [
    ("Q A D", "TOP LEFT, BOTTOM LEFT, RIGHT"),
    ("H P C R", "HEART, POWER, SPD/CAD, RADAR"),
    ("B", "BLE HEART STRAP"),
    ("G", "GPS FIX"),
    ("L", "BATTERY LEVEL"),
    ("K", "CHARGING"),
    ("V", "VEHICLE APPROACHES"),
    ("X", "POWER LOSS"),
    ("S", "SAVE CHECKPOINT"),
    ("SPACE", "PAUSE"),
    ("1 2 3", "1X, 10X, 60X"),
    ("ESC", "QUIT AND SAVE"),
];

fn button(key: char) -> Option<Button> {
    match key {
        'q' => Some(Button::TopLeft),
        'a' => Some(Button::BottomLeft),
        'd' => Some(Button::BottomRight),
        _ => None,
    }
}

struct Held {
    key: char,
    button: Button,
    real_ms: f64,
    /// Repeats follow real time, so a hold feels the same at every speed.
    next_real_ms: f64,
}

pub struct Live {
    pub sim: Sim,
    pub speed: u32,
    pub paused: bool,
    held: Option<Held>,
    owed_ms: f64,
    battery: usize,
    checkpoints: usize,
}

impl Live {
    /// A rider on a loop route with ANT sensors around, powering the device on.
    pub fn new() -> Self {
        // Each session keeps its own directory; relaunching never replaces one.
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut sim = Sim::named(&format!("live_session-{started}"));
        for (_, _, kind, number) in SENSORS {
            sim.ant_sensor(kind, number);
        }
        sim.ride_route(Route::loop_around(
            Point::new(-33.45, -70.66),
            5_000.0,
            20.0,
        ));
        sim.ride_at(25.0, 180.0, 130.0, 85.0);
        sim.set_battery(BATTERY[0], false);
        Self {
            sim,
            speed: 1,
            paused: false,
            held: None,
            owed_ms: 0.0,
            battery: 0,
            checkpoints: 0,
        }
    }

    pub fn key_down(&mut self, key: char) {
        if let Some(button) = button(key) {
            self.sim.note(NoteKind::Input, format!("press {button:?}"));
            self.sim.report(button);
            self.held = Some(Held {
                key,
                button,
                real_ms: 0.0,
                next_real_ms: HOLD_AFTER_MS,
            });
            return;
        }
        let sim = &mut self.sim;
        match key {
            ' ' => self.paused = !self.paused,
            '1'..='3' => {
                self.speed = SPEEDS[key as usize - '1' as usize];
                self.paused = false;
            }
            's' => {
                self.checkpoints += 1;
                sim.checkpoint(&format!("moment {}", self.checkpoints));
            }
            'b' => {
                let present = sim.device.ble_strap.as_ref().is_some_and(|s| s.present);
                sim.ble_heart_strap(!present);
            }
            'g' => {
                let fix = sim.device.gps.as_ref().is_some_and(|g| g.fix);
                sim.set_gps_fix(!fix);
            }
            'l' => {
                self.battery = (self.battery + 1) % BATTERY.len();
                let charging = sim.device.battery.charging;
                sim.set_battery(BATTERY[self.battery], charging);
            }
            'k' => {
                let charging = sim.device.battery.charging;
                sim.set_battery(BATTERY[self.battery], !charging);
            }
            'v' => sim.vehicle(Vehicle {
                range_m: 150.0,
                closing_kmh: 25.0,
                fast: false,
                side: Side::Behind,
            }),
            'x' => sim.cut_power(),
            _ => {
                if let Some(&(_, _, kind, number)) = SENSORS.iter().find(|s| s.0 == key) {
                    let present = sim.device.ant.sensor(kind, number).present;
                    sim.set_present(kind, number, !present);
                }
            }
        }
    }

    pub fn key_up(&mut self, key: char) {
        if self.held.as_ref().is_some_and(|held| held.key == key) {
            self.held = None;
        }
    }

    /// Let `real_ms` of wall time pass at the current speed, in firmware passes.
    pub fn tick(&mut self, real_ms: f64) {
        if let Some(held) = &mut self.held {
            held.real_ms += real_ms;
        }
        if self.paused {
            return;
        }
        if let Some(held) = &mut self.held
            && held.real_ms >= held.next_real_ms
        {
            held.next_real_ms = held.real_ms + HOLD_REPEAT_MS as f64;
            self.sim.repeat(held.button);
        }
        // While a key is held the device runs in real time, as the companion
        // repeats every 100 ms of device time: VANA sees one continuous hold,
        // not presses seconds apart, at any selected speed.
        let speed = if self.held.is_some() {
            1.0
        } else {
            f64::from(self.speed)
        };
        self.owed_ms = (self.owed_ms + real_ms * speed).min(MAX_OWED_REAL_MS * speed);
        while self.owed_ms >= STEP_MS as f64 {
            self.owed_ms -= STEP_MS as f64;
            self.sim.advance(STEP_MS);
        }
    }

    /// The device screen beside a panel of clock, world, toggles and keys.
    pub fn compose(&self) -> Canvas {
        let frame = self.sim.device.display.0.borrow();
        let (w, h) = (frame.geometry.width, frame.geometry.height);
        let (screen_w, screen_h) = (w * SCALE, h * SCALE);
        let width = MARGIN * 3 + screen_w + PANEL_WIDTH;
        let height = MARGIN * 2 + screen_h + 40;
        let mut canvas = Canvas::new(width, height, BACKGROUND);
        canvas.fill_rect(MARGIN - 6, MARGIN - 6, screen_w + 12, screen_h + 12, BEZEL);
        canvas.blit565(MARGIN, MARGIN, w, h, &frame.pixels, SCALE);
        drop(frame);

        let sim = &self.sim;
        let world = &sim.device.world;
        let x = MARGIN * 2 + screen_w;
        let mut y = MARGIN;
        canvas.text(x, y, 2, INK, "LIVE SESSION");
        y += 26;
        canvas.text(x, y, 3, ACCENT, &format!("T+{}", clock(world.time_ms)));
        y += 30;
        let (rate, color) = if self.paused {
            ("PAUSED".to_string(), AMBER)
        } else {
            (format!("{}X", self.speed), INK)
        };
        canvas.text(x, y, 2, color, &rate);
        y += 28;
        canvas.text(x, y, 1, DIM, "WORLD");
        let line = format!(
            "{:.0} KM/H  {:.0} W  {:.0} BPM",
            world.speed_kmh, world.power_w, world.heart_bpm
        );
        canvas.text(x, y + 10, 2, INK, &line);
        y += 30;
        canvas.text(
            x,
            y,
            1,
            INK,
            &format!(
                "{:.0} RPM  {:.2} KM",
                world.cadence_rpm,
                world.distance_m / 1000.0
            ),
        );
        y += 16;

        let mut status = |label: String, on: bool| {
            canvas.text(x, y, 1, if on { PASS } else { FAIL }, &label);
            y += 11;
        };
        for (_, label, kind, number) in SENSORS {
            let present = sim.device.ant.sensors.iter().any(|s| {
                s.identity.device_type == kind && s.identity.device_number == number && s.present
            });
            status(
                format!("{label} {number} {}", if present { "ON" } else { "OFF" }),
                present,
            );
        }
        let strap = sim.device.ble_strap.as_ref().is_some_and(|s| s.present);
        status(
            format!("BLE STRAP {}", if strap { "ON" } else { "OFF" }),
            strap,
        );
        let fix = sim.device.gps.as_ref().is_some_and(|g| g.fix);
        status(format!("GPS {}", if fix { "FIX" } else { "NO FIX" }), fix);
        let battery = &sim.device.battery;
        let percent = battery.level.map_or(0, |(percent, _)| percent);
        status(
            format!(
                "BATTERY {percent}%{}",
                if battery.charging { " CHARGING" } else { "" }
            ),
            percent > 15 || battery.charging,
        );
        let nearest = world
            .vehicles
            .iter()
            .map(|v| v.range_m)
            .min_by(f64::total_cmp);
        status(
            match nearest {
                Some(range) => format!("VEHICLE {range:.0} M BEHIND"),
                None => "NO VEHICLES".to_string(),
            },
            nearest.is_none(),
        );
        y += 8;

        canvas.text(x, y, 1, DIM, "KEYS");
        y += 12;
        for (keys, action) in KEYS {
            canvas.text(x, y, 1, ACCENT, keys);
            canvas.text(x + 60, y, 1, INK, action);
            y += 11;
        }
        y += 8;

        canvas.text(x, y, 1, DIM, "EVENTS");
        y += 12;
        let recent: Vec<_> = sim.notes.iter().rev().take(8).collect();
        for note in recent.iter().rev() {
            if y + 20 > height - 40 {
                break;
            }
            let line = format!("{} {}", clock(note.at_ms), note.text);
            y += wrap(&mut canvas, x, y, 1, DIM, &line, PANEL_WIDTH) + 2;
        }
        canvas.text(MARGIN, height - 30, 2, DIM, "VANA ON SIMULATED C606");
        canvas
    }
}

impl Default for Live {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(live: &mut Live, ms: u64) {
        for _ in 0..ms / 33 {
            live.tick(33.0);
        }
    }

    #[test]
    fn boots_in_real_time_and_composes_screen_and_panel() {
        let mut live = Live::new();
        run(&mut live, 8_000);
        assert!(live.sim.screen().contains("READY TO RIDE"));
        let canvas = live.compose();
        assert_eq!(canvas.width, MARGIN * 3 + 240 * SCALE + PANEL_WIDTH);
        assert_eq!(canvas.height, MARGIN * 2 + 320 * SCALE + 40);
        assert!(canvas.rgb.chunks(3).any(|p| p == ACCENT));
    }

    #[test]
    fn speed_pause_and_toggles_drive_the_world() {
        let mut live = Live::new();
        live.key_down('3');
        live.tick(30.0);
        assert_eq!(live.sim.now(), 1_800);
        live.key_down(' ');
        live.tick(30.0);
        assert_eq!(live.sim.now(), 1_800);
        live.key_down('h');
        assert!(
            !live
                .sim
                .device
                .ant
                .sensor(device_type::HEART_RATE, 4242)
                .present
        );
        live.key_down('g');
        assert!(!live.sim.device.gps.as_ref().unwrap().fix);
        live.key_down('k');
        assert!(live.sim.device.battery.charging);
    }

    /// Moves of the Home cursor while `key` stays down for `ms`.
    fn moves(live: &mut Live, key: char, ms: u64) -> usize {
        // The selected Home tile is drawn black on cyan.
        let train = |live: &Live| {
            let screen = live.sim.screen();
            ["TRAIN", "SENSORS", "SETTINGS"]
                .into_iter()
                .position(|tile| screen.run(tile).unwrap().color == 0)
        };
        let mut was = train(live);
        let mut moves = 0;
        live.key_down(key);
        for _ in 0..ms / 33 {
            live.tick(33.0);
            moves += usize::from(train(live) != was);
            was = train(live);
        }
        live.key_up(key);
        run(live, 1_000);
        moves + usize::from(train(live) != was)
    }

    #[test]
    fn a_tap_is_one_press_and_a_hold_repeats() {
        let mut live = Live::new();
        run(&mut live, 8_000);
        assert_eq!(moves(&mut live, 'a', 200), 1);
        assert!(moves(&mut live, 'a', 1_500) >= 3);
    }

    #[test]
    fn a_hold_repeats_in_real_time_at_every_speed() {
        let mut live = Live::new();
        run(&mut live, 8_000);
        live.key_down('3');
        // About eight repeats in a real second, not one per simulated 100 ms.
        let moved = moves(&mut live, 'a', 1_000);
        assert!((3..=10).contains(&moved), "{moved} moves");
    }

    #[test]
    fn holding_q_turns_an_off_device_back_on() {
        let mut live = Live::new();
        run(&mut live, 8_000);
        for key in ['q', 'd'] {
            live.key_down(key);
            live.key_up(key);
            run(&mut live, 600);
        }
        run(&mut live, 2_000);
        assert!(live.sim.is_off(), "the turn-off sheet did not turn it off");
        live.key_down('q');
        live.key_up('q');
        run(&mut live, 1_000);
        assert!(live.sim.is_off(), "a tap turned it on");
        live.key_down('q');
        run(&mut live, 2_500);
        live.key_up('q');
        assert!(!live.sim.is_off(), "holding Q did not turn it on");
        run(&mut live, 8_000);
        assert!(live.sim.screen().contains("READY TO RIDE"));
    }

    #[test]
    fn holding_q_turns_an_off_device_on_at_every_speed() {
        for speed in SPEEDS {
            let mut live = Live::new();
            run(&mut live, 8_000);
            for key in ['q', 'd'] {
                live.key_down(key);
                live.key_up(key);
                run(&mut live, 600);
            }
            run(&mut live, 2_000);
            assert!(live.sim.is_off(), "{speed}x: the sheet did not turn it off");
            live.speed = speed;
            live.key_down('q');
            // The rest of the power-on hold never reads as BACK on Home.
            for _ in 0..2_500 / 33 {
                live.tick(33.0);
                let screen = live.sim.screen();
                assert!(
                    !screen.contains("TURN OFF"),
                    "{speed}x: the hold opened the sheet"
                );
            }
            live.key_up('q');
            assert!(!live.sim.is_off(), "{speed}x: holding Q did not turn it on");
            run(&mut live, 6_000);
            let screen = live.sim.screen();
            assert!(
                !screen.contains("TURN OFF"),
                "{speed}x: the hold opened the sheet"
            );
            assert!(
                screen.contains("READY TO RIDE"),
                "{speed}x:\n{}",
                screen.text()
            );
        }
    }

    #[test]
    fn a_held_key_runs_the_device_in_real_time_then_the_speed_returns() {
        let mut live = Live::new();
        run(&mut live, 8_000);
        live.speed = 60;
        let start = live.sim.device.now;
        live.key_down('a');
        run(&mut live, 1_000);
        let held = live.sim.device.now - start;
        assert!(
            (900..=1_100).contains(&held),
            "{held} ms of uptime in a real second"
        );
        live.key_up('a');
        let start = live.sim.device.now;
        run(&mut live, 1_000);
        let free = live.sim.device.now - start;
        assert!(free >= 50_000, "{free} ms of uptime at 60x");
    }

    #[test]
    fn a_slow_frame_owes_at_most_a_quarter_second_of_real_time() {
        let mut live = Live::new();
        live.key_down('3');
        let before = live.sim.now();
        live.tick(5_000.0);
        assert_eq!(live.sim.now() - before, 15_000);
    }
}
