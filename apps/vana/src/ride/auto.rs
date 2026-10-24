//! Auto-pause and auto-start policy, decided from the ride's speed source.
//!
//! Pure state machine: it never touches the recorder. The caller feeds it the
//! recorder status and the current `Motion` once per tick and performs the
//! returned `Decision` through the ordinary request path, so an automatic
//! action is journaled as the same Pause, Resume and Start events a button
//! press produces and every storage rule applies unchanged.
//!
//! Motion comes from a wheel sensor when one measures, otherwise from GPS
//! (`ride::gps_speed`). Each source has its own `Rules`: a wheel measures a
//! standstill exactly, while GPS lags by a second or two and jitters by
//! about a metre at rest, so GPS needs a faster speed held longer before it
//! resumes or starts a ride. Only a measured speed pauses, resumes or starts
//! a ride. An unmeasured but live source (a wheel not yet turning, a fix with
//! poor geometry) changes nothing, and an auto-pause whose sources are all
//! lost resumes after the timeout of the source it was measured by, rather
//! than stay paused forever. The decision table test lists every combination.

use crate::ride::log::Status;

/// Auto-pause and auto-start are on by default; a settings screen can drive this.
pub const ENABLED: bool = true;

/// Where a speed comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Wheel,
    Gps,
}

/// Thresholds for one speed source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rules {
    /// Below this the rider counts as stopped (mm/s).
    pub pause_below: u32,
    /// The rider must stay stopped this long before the ride pauses. The
    /// debounce itself stays in active time; the recorder cannot back-date it.
    pub pause_after: u64,
    /// At or above this a paused ride is moving again. The gap to
    /// `pause_below` is the hysteresis that keeps a crawl from toggling.
    pub resume_at: u32,
    /// The speed must hold this long to resume, so a single glitch does not.
    pub resume_after: u64,
    /// At or above this a ride that has not started begins.
    pub start_at: u32,
    /// The speed must hold this long to start.
    pub start_after: u64,
    /// An auto-pause measured by this source resumes after it has been lost
    /// this long.
    pub lost_resume_after: u64,
}

/// A wheel sensor: 3 km/h stops, 4.5 km/h resumes, 6 km/h starts.
pub const WHEEL: Rules = Rules {
    pause_below: 833,
    pause_after: 1_000,
    resume_at: 1_250,
    resume_after: 1_000,
    start_at: 1_667,
    start_after: 3_000,
    lost_resume_after: 3_000,
};

/// GPS: standing reads zero below 3.6 km/h (`gps_speed::STOPPED_BELOW_MM_S`).
/// 7.2 km/h held 2 s resumes, which standstill jitter does not reach; 10 km/h
/// held 5 s starts, so walking the bike out of the door does not. A lost fix
/// (a tunnel, a garage) resumes only after 10 s.
pub const GPS: Rules = Rules {
    pause_below: 1_000,
    pause_after: 2_000,
    resume_at: 2_000,
    resume_after: 2_000,
    start_at: 2_778,
    start_after: 5_000,
    lost_resume_after: 10_000,
};

impl Source {
    pub const fn rules(self) -> Rules {
        match self {
            Self::Wheel => WHEEL,
            Self::Gps => GPS,
        }
    }
}

/// What the speed sources tell the ride right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Motion {
    /// No source can be heard: no wheel sensor (or not connected, stale) and
    /// no fresh GPS fix.
    Lost,
    /// A live source with no speed established yet: a wheel just connected or
    /// one turn after a long stand, or a fix with poor geometry.
    Unknown(Source),
    /// A measured speed in mm/s; zero when standing.
    Measured(Source, u32),
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
    /// The source of the last measured speed, whose loss timeout applies.
    measured_by: Option<Source>,
    /// Since when the condition for the next decision has held, for which
    /// source (`None` while lost) and decision.
    since: Option<(u64, Option<Source>, Decision)>,
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
        motion: Motion,
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
        if let Motion::Measured(source, _) = motion {
            self.measured_by = Some(source);
        }
        let source = match motion {
            Motion::Lost => None,
            Motion::Unknown(source) | Motion::Measured(source, _) => Some(source),
        };
        let speed = match motion {
            Motion::Measured(_, speed) => Some(speed),
            _ => None,
        };
        let rules = source.unwrap_or(Source::Wheel).rules();
        let (condition, hold, decision) = match status {
            Status::Recording if !self.paused => (
                speed.is_some_and(|speed| speed < rules.pause_below),
                rules.pause_after,
                Decision::Pause,
            ),
            Status::Paused if self.paused => match source {
                // A source that cannot be heard cannot tell a stop from a dropout.
                None => {
                    let lost = self.measured_by.unwrap_or(Source::Wheel).rules();
                    (true, lost.lost_resume_after, Decision::Resume)
                }
                // Alive but not measured: stay paused until it is.
                Some(_) => (
                    speed.is_some_and(|speed| speed >= rules.resume_at),
                    rules.resume_after,
                    Decision::Resume,
                ),
            },
            Status::Ready if ready => (
                speed.is_some_and(|speed| speed >= rules.start_at),
                rules.start_after,
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
        // A hold timed for one source or decision does not carry over.
        if self
            .since
            .is_some_and(|(_, was_source, was)| (was_source, was) != (source, decision))
        {
            self.since = None;
        }
        let (since, _, _) = *self.since.get_or_insert((now, source, decision));
        (now.saturating_sub(since) >= hold).then_some(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOPPED: Motion = Motion::Measured(Source::Wheel, 0);
    const CRAWL: Motion = Motion::Measured(Source::Wheel, 1_000);
    const RIDING: Motion = Motion::Measured(Source::Wheel, 6_000);
    const UNKNOWN: Motion = Motion::Unknown(Source::Wheel);

    /// Feed a constant reading every 100 ms until the first decision.
    fn run(
        auto: &mut Auto,
        status: Status,
        wheel: Motion,
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
        type Row = (Status, bool, Motion, Option<(Decision, u64)>);
        let rows: &[Row] = &[
            // Recording: only a measured stop pauses, after the debounce.
            (Status::Recording, false, RIDING, None),
            (Status::Recording, false, CRAWL, None),
            (
                Status::Recording,
                false,
                STOPPED,
                hold(WHEEL.pause_after).map(|t| (Pause, t)),
            ),
            (Status::Recording, false, UNKNOWN, None),
            (Status::Recording, false, Motion::Lost, None),
            // Auto-paused: movement resumes, a crawl or unknown holds, a lost
            // sensor resumes after its own timeout.
            (
                Status::Paused,
                true,
                RIDING,
                Some((Resume, WHEEL.resume_after)),
            ),
            (Status::Paused, true, CRAWL, None),
            (Status::Paused, true, STOPPED, None),
            (Status::Paused, true, UNKNOWN, None),
            (
                Status::Paused,
                true,
                Motion::Lost,
                Some((Resume, WHEEL.lost_resume_after)),
            ),
            // Manual pause: automation never touches it.
            (Status::Paused, false, RIDING, None),
            (Status::Paused, false, STOPPED, None),
            (Status::Paused, false, UNKNOWN, None),
            (Status::Paused, false, Motion::Lost, None),
            // Ready: only a measured start speed starts.
            (
                Status::Ready,
                false,
                RIDING,
                Some((Start, WHEEL.start_after)),
            ),
            (Status::Ready, false, CRAWL, None),
            (Status::Ready, false, STOPPED, None),
            (Status::Ready, false, UNKNOWN, None),
            (Status::Ready, false, Motion::Lost, None),
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
            for wheel in [RIDING, STOPPED, UNKNOWN, Motion::Lost] {
                let mut auto = auto_paused();
                assert_eq!(run(&mut auto, status, wheel, 0, 10_000), None);
            }
        }
    }

    const GPS_STOPPED: Motion = Motion::Measured(Source::Gps, 0);
    /// Faster than the wheel's resume speed, slower than GPS needs.
    const GPS_ROLLING: Motion = Motion::Measured(Source::Gps, 1_600);
    const GPS_RIDING: Motion = Motion::Measured(Source::Gps, 6_000);
    const GPS_POOR: Motion = Motion::Unknown(Source::Gps);

    /// The same table for GPS, and a source that was lost after each one.
    #[test]
    fn gps_decision_table() {
        use Decision::{Pause, Resume, Start};
        // (status, paused by auto, last measured by, motion, expected)
        type Row = (
            Status,
            bool,
            Option<Source>,
            Motion,
            Option<(Decision, u64)>,
        );
        let rows: &[Row] = &[
            (Status::Recording, false, None, GPS_RIDING, None),
            (Status::Recording, false, None, GPS_ROLLING, None),
            (
                Status::Recording,
                false,
                None,
                GPS_STOPPED,
                Some((Pause, GPS.pause_after)),
            ),
            (Status::Recording, false, None, GPS_POOR, None),
            (
                Status::Paused,
                true,
                None,
                GPS_RIDING,
                Some((Resume, GPS.resume_after)),
            ),
            // Rolling at 5.8 km/h: a wheel would resume, GPS jitter could too.
            (Status::Paused, true, None, GPS_ROLLING, None),
            (Status::Paused, true, None, GPS_STOPPED, None),
            (Status::Paused, true, None, GPS_POOR, None),
            (Status::Paused, false, None, GPS_RIDING, None),
            (
                Status::Paused,
                true,
                Some(Source::Gps),
                Motion::Lost,
                Some((Resume, GPS.lost_resume_after)),
            ),
            (
                Status::Paused,
                true,
                Some(Source::Wheel),
                Motion::Lost,
                Some((Resume, WHEEL.lost_resume_after)),
            ),
            (
                Status::Ready,
                false,
                None,
                GPS_RIDING,
                Some((Start, GPS.start_after)),
            ),
            (Status::Ready, false, None, GPS_ROLLING, None),
            (Status::Ready, false, None, GPS_POOR, None),
        ];
        for &(status, by_auto, measured_by, motion, expected) in rows {
            let mut auto = Auto::default();
            if let Some(source) = measured_by {
                auto.update(0, Status::Saved, Motion::Measured(source, 0), true);
            }
            if by_auto {
                auto.accepted(Decision::Pause);
            }
            let got = run(&mut auto, status, motion, 0, 20_000).map(|(at, d)| (d, at));
            assert_eq!(
                got, expected,
                "{status:?} auto={by_auto} {measured_by:?} {motion:?}"
            );
        }
    }

    #[test]
    fn gps_at_a_traffic_light_pauses_holds_through_jitter_and_resumes() {
        let mut auto = Auto::default();
        assert_eq!(
            run(&mut auto, Status::Recording, GPS_RIDING, 0, 30_000),
            None
        );
        let paused = run(&mut auto, Status::Recording, GPS_STOPPED, 30_000, 10_000);
        assert_eq!(paused, Some((32_000, Decision::Pause)));
        auto.accepted(Decision::Pause);
        // Jitter: a second at a riding speed now and then, poor geometry.
        for second in 0..63u64 {
            let motion = match second % 7 {
                3 => GPS_RIDING,
                5 => GPS_POOR,
                _ => GPS_STOPPED,
            };
            let from = 33_000 + second * 1000;
            assert_eq!(run(&mut auto, Status::Paused, motion, from, 1_000), None);
        }
        assert!(auto.paused());
        let resumed = run(&mut auto, Status::Paused, GPS_RIDING, 100_000, 10_000);
        assert_eq!(resumed, Some((102_000, Decision::Resume)));
    }

    #[test]
    fn a_wheel_dropout_hands_over_to_gps_with_its_own_hold() {
        let mut auto = auto_paused();
        // Wheel says riding for half a second, then drops: GPS starts its hold.
        assert_eq!(run(&mut auto, Status::Paused, RIDING, 0, 500), None);
        assert_eq!(run(&mut auto, Status::Paused, GPS_RIDING, 500, 1_900), None);
        assert_eq!(
            run(&mut auto, Status::Paused, GPS_RIDING, 2_400, 1_000),
            Some((2_500, Decision::Resume))
        );
    }

    #[test]
    fn a_stand_then_one_turn_stays_paused_while_the_wheel_is_unknown() {
        // Fresh packets, speed not measured yet (one turn after a long stand):
        // neither a resume nor the sensor-loss timeout may fire.
        let mut auto = auto_paused();
        assert_eq!(run(&mut auto, Status::Paused, UNKNOWN, 0, 30_000), None);
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
        auto.manual();
        assert!(!auto.paused());
        assert_eq!(run(&mut auto, Status::Paused, RIDING, 0, 60_000), None);
    }

    #[test]
    fn a_dropout_restarts_the_stop_timer() {
        let mut auto = Auto::default();
        assert_eq!(run(&mut auto, Status::Recording, STOPPED, 0, 900), None);
        assert_eq!(
            run(&mut auto, Status::Recording, Motion::Lost, 900, 100),
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
        let five = Motion::Measured(Source::Wheel, 1_388);
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
        assert_eq!(run(&mut auto, Status::Paused, Motion::Lost, 0, 1_500), None);
        assert_eq!(run(&mut auto, Status::Paused, RIDING, 1_500, 900), None);
        assert_eq!(
            run(&mut auto, Status::Paused, RIDING, 2_400, 1_000),
            Some((2_500, Decision::Resume))
        );
        // Moving time does not count toward the sensor-loss timeout either.
        let mut auto = auto_paused();
        assert_eq!(run(&mut auto, Status::Paused, CRAWL, 0, 2_500), None);
        assert_eq!(
            run(&mut auto, Status::Paused, Motion::Lost, 2_500, 2_900),
            None
        );
    }
}
