//! Auto-pause and auto-start policy, decided from the wheel speed alone.
//!
//! Pure state machine: it never touches the recorder. The caller feeds it the
//! recorder status and the current speed once per tick and performs the
//! returned `Decision` through the ordinary request path, so an automatic
//! action is journaled as the same Pause, Resume and Start events a button
//! press produces and every storage rule applies unchanged.
//!
//! The input is the `Wheel` state: a measured speed, a live sensor whose speed
//! is not established yet, or a sensor that cannot be heard (not connected,
//! stale, or none; the app has no GPS speed). Only a measured speed pauses,
//! resumes or starts a ride. An unmeasured but live sensor changes nothing, and
//! an auto-pause whose sensor is truly lost resumes rather than stay paused
//! forever. The decision table test in this file lists every combination.

use crate::ride::log::Status;

/// Auto-pause and auto-start are on by default; a settings screen can drive this.
pub const ENABLED: bool = true;

/// Below this the rider counts as stopped: 3 km/h in mm/s.
pub const PAUSE_BELOW_MM_S: u32 = 833;
/// The rider must stay stopped this long before the ride pauses. The debounce
/// itself stays in active time; the recorder cannot back-date a pause.
pub const PAUSE_AFTER_MS: u64 = 1_000;
/// At or above this a paused ride is moving again: 4.5 km/h in mm/s. The gap to
/// `PAUSE_BELOW_MM_S` is the hysteresis that keeps a crawl from toggling.
pub const RESUME_AT_MM_S: u32 = 1_250;
/// The speed must hold for this long to resume, so a single glitch does not.
pub const RESUME_AFTER_MS: u64 = 1_000;
/// At or above this a ride that has not started begins: 6 km/h in mm/s.
pub const START_AT_MM_S: u32 = 1_667;
/// The speed must hold for this long to start.
pub const START_AFTER_MS: u64 = 3_000;
/// An auto-pause without a speed reading for this long resumes.
pub const SENSOR_LOST_RESUME_MS: u64 = 3_000;

/// What the wheel sensor tells the ride right now. Only `Lost` means the
/// sensor cannot be heard; `Unknown` is a live sensor whose speed is not
/// established yet (just connected, or one turn after a long stand).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wheel {
    /// Not connected, stale, or no wheel sensor at all (GPS is not a source).
    Lost,
    /// Connected and sending fresh packets, with no speed measured yet.
    Unknown,
    /// A measured speed in mm/s; zero when the wheel stands.
    Measured(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Pause,
    Resume,
    Start,
}

#[derive(Default)]
pub struct Auto {
    /// The ride is paused by automation (or about to be). A manual pause is
    /// never resumed automatically.
    paused: bool,
    /// Since when the condition for the next decision has held.
    since: Option<(u64, bool, Decision)>,
}

impl Auto {
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The rider took control; automation lets go of the pause.
    pub fn manual(&mut self) {
        self.paused = false;
        self.since = None;
    }

    /// Rider presses ACT while the ride is auto-paused: the pause becomes a
    /// manual one and stays. Reports whether that happened.
    pub fn take_over(&mut self) -> bool {
        let was = self.paused;
        self.manual();
        was
    }

    /// A requested decision was refused by the recorder.
    pub fn declined(&mut self) {
        self.since = None;
    }

    /// A requested decision was accepted by the recorder.
    pub fn accepted(&mut self, decision: Decision) {
        self.since = None;
        self.paused = decision == Decision::Pause;
    }

    /// Advance one tick. `ready` is whether a ride may start now (ride screen
    /// shown, nothing pending).
    pub fn update(
        &mut self,
        now: u64,
        status: Status,
        wheel: Wheel,
        ready: bool,
    ) -> Option<Decision> {
        if !ENABLED {
            return None;
        }
        // The caller skips ticks with a request in flight, so a ride that is
        // recording is not about to pause: any automatic pause is over.
        if status != Status::Paused {
            self.paused = false;
        }
        let mut lost = false;
        let (condition, hold, decision) = match status {
            Status::Recording if !self.paused => (
                matches!(wheel, Wheel::Measured(speed) if speed < PAUSE_BELOW_MM_S),
                PAUSE_AFTER_MS,
                Decision::Pause,
            ),
            Status::Paused if self.paused => match wheel {
                // A sensor that cannot be heard cannot tell a stop from a dropout.
                Wheel::Lost => {
                    lost = true;
                    (true, SENSOR_LOST_RESUME_MS, Decision::Resume)
                }
                // Alive but not yet measured: stay paused until it is.
                Wheel::Unknown => (false, RESUME_AFTER_MS, Decision::Resume),
                Wheel::Measured(speed) => {
                    (speed >= RESUME_AT_MM_S, RESUME_AFTER_MS, Decision::Resume)
                }
            },
            Status::Ready if ready => (
                matches!(wheel, Wheel::Measured(speed) if speed >= START_AT_MM_S),
                START_AFTER_MS,
                Decision::Start,
            ),
            _ => {
                self.since = None;
                return None;
            }
        };
        if !condition {
            self.since = None;
            return None;
        }
        // A hold timed for one condition does not carry over to the other.
        if self
            .since
            .is_some_and(|(_, was_lost, was)| (was_lost, was) != (lost, decision))
        {
            self.since = None;
        }
        let (since, _, _) = *self.since.get_or_insert((now, lost, decision));
        (now.saturating_sub(since) >= hold).then_some(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOPPED: Wheel = Wheel::Measured(0);
    const CRAWL: Wheel = Wheel::Measured(1_000);
    const RIDING: Wheel = Wheel::Measured(6_000);

    /// Feed a constant reading every 100 ms until the first decision.
    fn run(
        auto: &mut Auto,
        status: Status,
        wheel: Wheel,
        from: u64,
        ms: u64,
    ) -> Option<(u64, Decision)> {
        (from..from + ms).step_by(100).find_map(|now| {
            auto.update(now, status, wheel, true)
                .map(|decision| (now, decision))
        })
    }

    fn auto_paused() -> Auto {
        let mut auto = Auto::default();
        auto.accepted(Decision::Pause);
        auto
    }

    /// Every combination of ride state, who paused and what the wheel says,
    /// held for ten seconds: what the policy decides, and when.
    #[test]
    fn decision_table() {
        use Decision::{Pause, Resume, Start};
        let hold = |ms| Some(ms);
        // (status, paused by auto, wheel, expected first decision and its time)
        type Row = (Status, bool, Wheel, Option<(Decision, u64)>);
        let rows: &[Row] = &[
            // Recording: only a measured stop pauses, after the debounce.
            (Status::Recording, false, RIDING, None),
            (Status::Recording, false, CRAWL, None),
            (
                Status::Recording,
                false,
                STOPPED,
                hold(PAUSE_AFTER_MS).map(|t| (Pause, t)),
            ),
            (Status::Recording, false, Wheel::Unknown, None),
            (Status::Recording, false, Wheel::Lost, None),
            // Auto-paused: movement resumes, a crawl or unknown holds, a lost
            // sensor resumes after its own timeout.
            (
                Status::Paused,
                true,
                RIDING,
                Some((Resume, RESUME_AFTER_MS)),
            ),
            (Status::Paused, true, CRAWL, None),
            (Status::Paused, true, STOPPED, None),
            (Status::Paused, true, Wheel::Unknown, None),
            (
                Status::Paused,
                true,
                Wheel::Lost,
                Some((Resume, SENSOR_LOST_RESUME_MS)),
            ),
            // Manual pause: automation never touches it.
            (Status::Paused, false, RIDING, None),
            (Status::Paused, false, STOPPED, None),
            (Status::Paused, false, Wheel::Unknown, None),
            (Status::Paused, false, Wheel::Lost, None),
            // Ready: only a measured start speed starts.
            (Status::Ready, false, RIDING, Some((Start, START_AFTER_MS))),
            (Status::Ready, false, CRAWL, None),
            (Status::Ready, false, STOPPED, None),
            (Status::Ready, false, Wheel::Unknown, None),
            (Status::Ready, false, Wheel::Lost, None),
        ];
        for &(status, by_auto, wheel, expected) in rows {
            let mut auto = Auto::default();
            if by_auto {
                auto.accepted(Decision::Pause);
            }
            let got = run(&mut auto, status, wheel, 0, 10_000).map(|(at, d)| (d, at));
            assert_eq!(got, expected, "{status:?} auto={by_auto} {wheel:?}");
        }
        // Other states never decide anything.
        for status in [
            Status::Saved,
            Status::Recovered,
            Status::Full,
            Status::Error,
            Status::Scanning,
            Status::NeedsInit,
        ] {
            for wheel in [RIDING, STOPPED, Wheel::Unknown, Wheel::Lost] {
                let mut auto = auto_paused();
                assert_eq!(run(&mut auto, status, wheel, 0, 10_000), None);
            }
        }
    }

    #[test]
    fn a_stand_then_one_turn_stays_paused_while_the_wheel_is_unknown() {
        // Fresh packets, speed not measured yet (one turn after a long stand):
        // neither a resume nor the sensor-loss timeout may fire.
        let mut auto = auto_paused();
        assert_eq!(
            run(&mut auto, Status::Paused, Wheel::Unknown, 0, 30_000),
            None
        );
        assert!(auto.paused());
    }

    #[test]
    fn pauses_after_one_second_stopped() {
        let mut auto = Auto::default();
        assert_eq!(run(&mut auto, Status::Recording, RIDING, 0, 10_000), None);
        let hit = run(&mut auto, Status::Recording, STOPPED, 10_000, 10_000);
        assert_eq!(hit, Some((11_000, Decision::Pause)));
    }

    #[test]
    fn a_brief_stop_does_not_pause() {
        let mut auto = Auto::default();
        assert_eq!(run(&mut auto, Status::Recording, STOPPED, 0, 900), None);
        assert_eq!(run(&mut auto, Status::Recording, RIDING, 900, 100), None);
        // The timer restarted: another 0.9 s is still not enough.
        assert_eq!(run(&mut auto, Status::Recording, STOPPED, 1_000, 900), None);
    }

    #[test]
    fn crawling_between_the_thresholds_neither_pauses_nor_resumes() {
        let mut auto = Auto::default();
        // 3.6 km/h is above the pause threshold.
        assert_eq!(run(&mut auto, Status::Recording, CRAWL, 0, 30_000), None);
        auto.accepted(Decision::Pause);
        // And below the resume threshold.
        assert_eq!(run(&mut auto, Status::Paused, CRAWL, 0, 30_000), None);
        assert!(auto.paused());
    }

    #[test]
    fn one_glitch_reading_does_not_resume() {
        let mut auto = auto_paused();
        assert_eq!(run(&mut auto, Status::Paused, RIDING, 0, 900), None);
        assert_eq!(run(&mut auto, Status::Paused, STOPPED, 900, 5_000), None);
    }

    #[test]
    fn taking_over_an_auto_pause_makes_it_manual() {
        let mut auto = auto_paused();
        assert!(auto.take_over());
        assert!(!auto.paused());
        assert_eq!(run(&mut auto, Status::Paused, RIDING, 0, 60_000), None);
        assert!(!auto.take_over());
    }

    #[test]
    fn a_dropout_restarts_the_stop_timer() {
        let mut auto = Auto::default();
        assert_eq!(run(&mut auto, Status::Recording, STOPPED, 0, 900), None);
        assert_eq!(
            run(&mut auto, Status::Recording, Wheel::Lost, 900, 100),
            None
        );
        assert_eq!(run(&mut auto, Status::Recording, STOPPED, 1_000, 900), None);
    }

    #[test]
    fn start_waits_for_the_ride_screen_and_is_retried_after_a_refusal() {
        let mut auto = Auto::default();
        let blocked = (0..10_000)
            .step_by(100)
            .find_map(|now| auto.update(now, Status::Ready, RIDING, false));
        assert_eq!(blocked, None);
        let first = run(&mut auto, Status::Ready, RIDING, 10_000, 5_000);
        assert_eq!(first, Some((13_000, Decision::Start)));
        // Storage refused: the hold starts over instead of retrying every tick.
        auto.declined();
        assert_eq!(auto.update(13_100, Status::Ready, RIDING, true), None);
        let retry = run(&mut auto, Status::Ready, RIDING, 13_200, 5_000);
        assert_eq!(retry, Some((16_100, Decision::Start)));
    }

    #[test]
    fn a_host_start_during_the_start_hold_gets_a_fresh_pause_debounce() {
        let mut auto = Auto::default();
        // 2.5 s into the auto-start hold the host starts the ride, and the
        // rider has stopped by then.
        assert_eq!(run(&mut auto, Status::Ready, RIDING, 0, 2_500), None);
        assert_eq!(
            run(&mut auto, Status::Recording, STOPPED, 2_500, 900),
            None,
            "the start hold's clock must not count toward the pause"
        );
        assert_eq!(
            run(&mut auto, Status::Recording, STOPPED, 3_400, 1_000),
            Some((3_500, Decision::Pause))
        );
    }

    #[test]
    fn five_kmh_is_not_enough_to_start() {
        let mut auto = Auto::default();
        let five = Wheel::Measured(1_388);
        assert_eq!(run(&mut auto, Status::Ready, five, 0, 20_000), None);
    }

    #[test]
    fn leaving_the_ride_forgets_the_automatic_pause() {
        let mut auto = auto_paused();
        assert_eq!(auto.update(0, Status::Saved, STOPPED, true), None);
        assert!(!auto.paused());
    }

    #[test]
    fn switching_between_lost_and_fast_restarts_the_hold() {
        let mut auto = auto_paused();
        // 1.5 s without a sensor, then fast: needs its own full second.
        assert_eq!(run(&mut auto, Status::Paused, Wheel::Lost, 0, 1_500), None);
        assert_eq!(run(&mut auto, Status::Paused, RIDING, 1_500, 900), None);
        assert_eq!(
            run(&mut auto, Status::Paused, RIDING, 2_400, 1_000),
            Some((2_500, Decision::Resume))
        );
        // Moving time does not count toward the sensor-loss timeout either.
        let mut auto = auto_paused();
        assert_eq!(run(&mut auto, Status::Paused, CRAWL, 0, 2_500), None);
        assert_eq!(
            run(&mut auto, Status::Paused, Wheel::Lost, 2_500, 2_900),
            None
        );
    }
}
