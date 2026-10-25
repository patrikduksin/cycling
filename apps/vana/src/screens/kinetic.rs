//! A list that follows the finger and coasts, with the motion of iOS's
//! `UIScrollView`.
//!
//! - Dragging moves the content with the finger, pixel for pixel, once the
//!   finger has travelled `DRAG_SLOP`; the slop is taken off, so nothing jumps.
//!   Past either end the content gives way less and less (the rubber band):
//!   an overshoot `x` shows as `(1 - 1 / (x · 0.55 / d + 1)) · d`, with `d` the
//!   viewport's height.
//! - Let go, it coasts at the finger's speed and slows by `DECELERATION` each
//!   millisecond (UIKit's `.normal`, 0.998), ending within half a pixel of
//!   where that curve rests.
//! - Coasting into an end, or let go past one, hands its velocity to a
//!   critically damped spring (mass 1, stiffness 100) that settles on the end
//!   without wobbling.
//! - A finger on moving content stops it where it is; that touch is no tap.
//!
//! The curves are the closed forms, evaluated at each frame's time, so the
//! motion is the same at any frame rate. Units are pixels and milliseconds.
//! The constants follow the published reconstruction of `UIScrollView`
//! (github.com/super-ultra/ScrollMechanics) and WWDC18 "Designing Fluid
//! Interfaces".
use libm::{expf, logf, powf};

/// Speed kept after each millisecond of coasting (`UIScrollView` `.normal`).
const DECELERATION: f32 = 0.998;
/// The rubber band's stiffness against the viewport's size.
const RUBBER: f32 = 0.55;
/// The bounce spring: mass 1, stiffness 100 /s², critically damped, so its
/// rate is √(k/m) = 10 /s, here per millisecond.
const BOUNCE_RATE: f32 = 0.010;
/// A focus change scrolls its card into view on a stiffer spring of the same
/// shape (stiffness 400): about a third of a second, as UIKit's animated
/// `scrollRectToVisible` takes.
const REVEAL_RATE: f32 = 0.020;
/// Motion ends this close to its rest.
const THRESHOLD: f32 = 0.5;
/// A spring this slow (px/ms) cannot move a pixel before it stops.
const SETTLED_SPEED: f32 = 0.01;
/// The finger travels this far before the content follows; less is a tap.
pub const DRAG_SLOP: i32 = 10;
/// The finger's speed is read over its last moves within this window...
const VELOCITY_WINDOW_MS: u64 = 100;
/// ...and is none when it last moved longer ago than that before lifting.
const STILL_MS: u64 = 100;
/// Moving content slower than this, in px/ms, stops under a finger without
/// swallowing the tap (20 px/s).
const CATCH_SPEED: f32 = 0.02;
/// Faster flicks than this are read at this speed, in px/ms.
const MAX_SPEED: f32 = 10.0;
/// Finger positions kept to read its speed.
const SAMPLES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Motion {
    Still(f32),
    /// A finger is down: `anchor` is where the content would sit, before the
    /// rubber band, with the finger at `finger`.
    Held {
        anchor: f32,
        finger: i32,
        dragging: bool,
        /// The finger stopped moving content.
        caught: bool,
    },
    /// Coasting from `from` at `velocity` (px/ms) since `at`.
    Coast {
        from: f32,
        velocity: f32,
        at: u64,
    },
    /// A critically damped spring onto `target` from `from` at `velocity`.
    Settle {
        target: f32,
        from: f32,
        velocity: f32,
        at: u64,
        rate: f32,
    },
}

/// The scroll offset of a list `viewport` pixels tall over `content` pixels:
/// 0 shows the top, `max` the bottom, and the rubber band reaches past both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kinetic {
    viewport: f32,
    max: f32,
    motion: Motion,
    /// The finger's last positions as (ms, y), oldest first.
    samples: [(u64, i32); SAMPLES],
    sampled: usize,
}

impl Kinetic {
    pub const fn new(viewport: usize) -> Self {
        Self {
            viewport: viewport as f32,
            max: 0.0,
            motion: Motion::Still(0.0),
            samples: [(0, 0); SAMPLES],
            sampled: 0,
        }
    }

    /// The content is now `content` pixels tall; content at rest past its
    /// new end springs back.
    pub fn set_content(&mut self, content: usize, now: u64) {
        self.max = (content as f32 - self.viewport).max(0.0);
        if let Motion::Still(at) = self.motion
            && at > self.max
        {
            self.motion = Motion::Still(self.max);
        }
        self.advance(now);
    }

    /// The offset to draw at `now`, in pixels: whole, so text stays crisp.
    pub fn offset(&self, now: u64) -> i32 {
        libm::roundf(self.state(now).0) as i32
    }

    /// Something still moves: draw the next frame soon.
    pub fn moving(&self) -> bool {
        matches!(
            self.motion,
            Motion::Coast { .. } | Motion::Settle { .. } | Motion::Held { dragging: true, .. }
        )
    }

    /// A finger is down on the list.
    pub fn held(&self) -> bool {
        matches!(self.motion, Motion::Held { .. })
    }

    /// Move a coast into its spring at an end, or end motion that has come
    /// to rest. Call before each frame.
    pub fn advance(&mut self, now: u64) {
        let (position, _, next) = self.state(now);
        if let Some(next) = next {
            self.motion = next;
            // A coast that hit an end goes on from there in its spring.
            if matches!(next, Motion::Settle { .. }) {
                self.advance(now);
            }
        } else if self.done(now) {
            self.motion = Motion::Still(position.clamp(0.0, self.max));
        }
    }

    /// A finger lands at height `y`. Moving content stops under it.
    pub fn press(&mut self, y: i32, now: u64) {
        let (position, velocity, _) = self.state(now);
        let caught = !matches!(self.motion, Motion::Still(_)) && velocity.abs() >= CATCH_SPEED;
        self.motion = Motion::Held {
            anchor: self.unband(position),
            finger: y,
            dragging: false,
            caught,
        };
        self.sampled = 0;
        self.sample(now, y);
    }

    /// The finger moved to height `y`.
    pub fn drag(&mut self, y: i32, now: u64) {
        let Motion::Held {
            anchor,
            finger,
            dragging,
            caught,
        } = self.motion
        else {
            return;
        };
        self.sample(now, y);
        if dragging {
            return;
        }
        // Past the slop the content takes up the finger from here on, less
        // the slop, so it starts moving from where it was.
        let travel = y - finger;
        if travel.abs() >= DRAG_SLOP {
            self.motion = Motion::Held {
                anchor,
                finger: finger + DRAG_SLOP * travel.signum(),
                dragging: true,
                caught,
            };
        }
    }

    /// The finger lifted at `now`. True when the touch scrolled or stopped
    /// the list, so it is no tap.
    pub fn release(&mut self, now: u64) -> bool {
        let Motion::Held {
            dragging, caught, ..
        } = self.motion
        else {
            return false;
        };
        let position = self.state(now).0;
        let velocity = if dragging {
            -self.finger_speed(now)
        } else {
            0.0
        };
        self.go(position, velocity, now);
        dragging || caught
    }

    /// The contact was lost: no fling, but content past an end goes back.
    pub fn cancel(&mut self, now: u64) {
        if self.held() {
            let position = self.state(now).0;
            self.go(position, 0.0, now);
        }
    }

    /// Bring `target` (clamped to the content) into view on the reveal
    /// spring, carrying whatever speed the content has.
    pub fn reveal(&mut self, target: f32, now: u64) {
        if self.held() {
            return;
        }
        let (from, velocity, _) = self.state(now);
        let target = target.clamp(0.0, self.max);
        self.motion = Motion::Settle {
            target,
            from,
            velocity,
            at: now,
            rate: REVEAL_RATE,
        };
        self.advance(now);
    }

    /// Jump to `target` at once.
    pub fn place(&mut self, target: f32) {
        self.motion = Motion::Still(target.clamp(0.0, self.max));
    }

    /// Where the list rests once its motion ends: the value `reveal` and the
    /// focus rules plan against.
    pub fn rest(&self) -> f32 {
        match self.motion {
            Motion::Still(at) => at,
            Motion::Held { .. } => self.state(0).0,
            Motion::Coast { from, velocity, .. } => {
                (from - velocity / logf(DECELERATION)).clamp(0.0, self.max)
            }
            Motion::Settle { target, .. } => target,
        }
    }

    /// Let go at `position` with `velocity`: coast, spring back from past an
    /// end, or stay.
    fn go(&mut self, position: f32, velocity: f32, now: u64) {
        let outside = position < 0.0 || position > self.max;
        self.motion = if outside {
            Motion::Settle {
                target: position.clamp(0.0, self.max),
                from: position,
                velocity,
                at: now,
                rate: BOUNCE_RATE,
            }
        } else if -velocity.abs() / logf(DECELERATION) >= THRESHOLD {
            Motion::Coast {
                from: position,
                velocity,
                at: now,
            }
        } else {
            Motion::Still(position)
        };
        self.advance(now);
    }

    /// Position (px), velocity (px/ms) and, when the motion has turned into
    /// another at `now`, that motion.
    fn state(&self, now: u64) -> (f32, f32, Option<Motion>) {
        match self.motion {
            Motion::Still(at) => (at, 0.0, None),
            Motion::Held {
                anchor,
                finger,
                dragging,
                ..
            } => {
                let y = self.latest().map_or(finger, |(_, y)| y);
                let raw = if dragging {
                    anchor - (y - finger) as f32
                } else {
                    anchor
                };
                (self.band(raw), 0.0, None)
            }
            Motion::Coast { from, velocity, at } => {
                let t = now.saturating_sub(at) as f32;
                let d = logf(DECELERATION);
                let position = from + (powf(DECELERATION, t) - 1.0) / d * velocity;
                let speed = velocity * powf(DECELERATION, t);
                // Coasting past an end hands its speed to the bounce spring
                // at the moment it crosses.
                let bound = if position < 0.0 {
                    Some(0.0)
                } else if position > self.max {
                    Some(self.max)
                } else {
                    None
                };
                match bound {
                    Some(bound) => {
                        let hit = logf(1.0 + d * (bound - from) / velocity) / d;
                        let hit = if hit.is_finite() { hit.max(0.0) } else { t };
                        let at_hit = velocity * powf(DECELERATION, hit);
                        let spring = Motion::Settle {
                            target: bound,
                            from: bound,
                            velocity: at_hit,
                            at: at + hit as u64,
                            rate: BOUNCE_RATE,
                        };
                        (position, speed, Some(spring))
                    }
                    None => (position, speed, None),
                }
            }
            Motion::Settle {
                target,
                from,
                velocity,
                at,
                rate,
            } => {
                let t = now.saturating_sub(at) as f32;
                let c1 = from - target;
                let c2 = velocity + rate * c1;
                let decay = expf(-rate * t);
                let position = target + decay * (c1 + c2 * t);
                let speed = decay * (c2 - rate * (c1 + c2 * t));
                (position, speed, None)
            }
        }
    }

    /// The motion has come within `THRESHOLD` of its rest.
    fn done(&self, now: u64) -> bool {
        match self.motion {
            Motion::Still(_) | Motion::Held { .. } => false,
            Motion::Coast { from, velocity, at } => {
                let rest = from - velocity / logf(DECELERATION);
                (self.state(now).0 - rest).abs() < THRESHOLD
                    || now.saturating_sub(at) as f32 >= coast_ms(velocity)
            }
            // Within half a pixel and too slow to leave it within a frame.
            Motion::Settle { target, .. } => {
                let (position, speed, _) = self.state(now);
                (position - target).abs() < THRESHOLD && speed.abs() < SETTLED_SPEED
            }
        }
    }

    /// `raw`, an offset that may lie past an end, as the rubber band shows it.
    fn band(&self, raw: f32) -> f32 {
        let d = self.viewport;
        let stretch = |x: f32| (1.0 - 1.0 / (x * RUBBER / d + 1.0)) * d;
        if raw < 0.0 {
            -stretch(-raw)
        } else if raw > self.max {
            self.max + stretch(raw - self.max)
        } else {
            raw
        }
    }

    /// The raw offset the rubber band shows at `shown`.
    fn unband(&self, shown: f32) -> f32 {
        let d = self.viewport;
        // The stretch never reaches `d`; keep its inverse finite.
        let unstretch = |y: f32| d / RUBBER * (1.0 / (1.0 - y.min(d * 0.99) / d) - 1.0);
        if shown < 0.0 {
            -unstretch(-shown)
        } else if shown > self.max {
            self.max + unstretch(shown - self.max)
        } else {
            shown
        }
    }

    fn sample(&mut self, now: u64, y: i32) {
        if self.sampled == SAMPLES {
            self.samples.copy_within(1.., 0);
            self.sampled -= 1;
        }
        self.samples[self.sampled] = (now, y);
        self.sampled += 1;
    }

    fn latest(&self) -> Option<(u64, i32)> {
        self.samples[..self.sampled].last().copied()
    }

    /// The finger's speed down the screen as it lifts, in px/ms: the slope
    /// of its last moves within `VELOCITY_WINDOW_MS`, by least squares, and
    /// none if it had stopped.
    fn finger_speed(&self, now: u64) -> f32 {
        let Some((last, _)) = self.latest() else {
            return 0.0;
        };
        if now.saturating_sub(last) > STILL_MS {
            return 0.0;
        }
        let recent = self.samples[..self.sampled]
            .iter()
            .filter(|(t, _)| last - t <= VELOCITY_WINDOW_MS);
        let (mut n, mut st, mut sy, mut stt, mut sty) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &(t, y) in recent {
            let (t, y) = ((last - t) as f32, y as f32);
            n += 1.0;
            st += t;
            sy += y;
            stt += t * t;
            sty += t * y;
        }
        let spread = n * stt - st * st;
        if n < 2.0 || spread <= 0.0 {
            return 0.0;
        }
        // `t` counts back from the last sample, so the slope's sign flips.
        let slope = -(n * sty - st * sy) / spread;
        slope.clamp(-MAX_SPEED, MAX_SPEED)
    }
}

/// How long a coast at `velocity` takes to come within `THRESHOLD` of rest.
fn coast_ms(velocity: f32) -> f32 {
    let d = logf(DECELERATION);
    let t = logf(-d * THRESHOLD / velocity.abs()) / d;
    if t.is_finite() { t.max(0.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list of 1000 px seen through 300: `max` is 700.
    fn list() -> Kinetic {
        let mut list = Kinetic::new(300);
        list.set_content(1000, 0);
        list
    }

    /// Drag from `from` to `to` over `ms`, a move every 10 ms (the C606's
    /// touch poll), starting at `start`; returns when the finger was last seen.
    fn flick(list: &mut Kinetic, from: i32, to: i32, start: u64, ms: u64) -> u64 {
        list.press(from, start);
        let steps = ms / 10;
        for step in 1..=steps {
            let y = from + (to - from) * step as i32 / steps as i32;
            list.drag(y, start + step * 10);
        }
        start + ms
    }

    fn run(list: &mut Kinetic, from: u64, to: u64) -> std::vec::Vec<f32> {
        (from..=to)
            .step_by(16)
            .map(|t| {
                list.advance(t);
                list.state(t).0
            })
            .collect()
    }

    #[test]
    fn the_content_follows_the_finger_once_past_the_slop_without_a_jump() {
        let mut list = list();
        list.place(100.0);
        list.press(200, 0);
        list.drag(195, 10);
        assert_eq!(list.offset(10), 100, "inside the slop it stays");
        assert!(!list.moving());
        list.drag(189, 20);
        assert_eq!(list.offset(20), 101, "the slop is taken off");
        list.drag(150, 30);
        assert_eq!(list.offset(30), 140, "then pixel for pixel");
        list.drag(250, 40);
        assert_eq!(list.offset(40), 40);
    }

    #[test]
    fn past_an_end_the_rubber_band_gives_way_less_and_less() {
        let mut list = list();
        list.press(0, 0);
        list.drag(10, 10);
        let pulled = |list: &mut Kinetic, by: i32| {
            list.drag(10 + by, 20);
            -list.state(20).0
        };
        // (1 - 1 / (x·0.55/300 + 1))·300.
        let expect = |x: f32| (1.0 - 1.0 / (x * 0.55 / 300.0 + 1.0)) * 300.0;
        for x in [10, 100, 300, 1000] {
            let shown = pulled(&mut list, x);
            assert!((shown - expect(x as f32)).abs() < 0.01, "{x}: {shown}");
            assert!(shown < x as f32 * 0.55 + 0.01);
        }
        assert!(pulled(&mut list, 100_000) < 300.0, "never a whole viewport");
        // Its inverse picks the drag up where the band left it.
        for shown in [-5.0, -80.0, 705.0, 850.0] {
            assert!((list.band(list.unband(shown)) - shown).abs() < 0.01);
        }
    }

    #[test]
    fn a_flick_coasts_on_uikits_normal_deceleration_and_rests_where_it_said() {
        let mut list = list();
        list.place(100.0);
        // 100 px up in 100 ms: 1 px/ms, the content moving down the list.
        let lift = flick(&mut list, 250, 150, 0, 100);
        assert!(list.release(lift), "a scroll is no tap");
        let Motion::Coast { from, velocity, .. } = list.motion else {
            panic!("{:?}", list.motion)
        };
        assert!((velocity - 1.0).abs() < 0.01, "{velocity}");
        assert_eq!(from, 190.0, "100 px of travel less the slop");
        // It rests v / -ln(0.998) ≈ 499.5 px on, as UIKit projects it.
        let rest = list.rest();
        assert!((rest - (190.0 + 499.5)).abs() < 1.0, "{rest}");
        // After 1 s it has slowed to 0.998^1000 ≈ 13.5 % of its speed.
        let (_, speed, _) = list.state(lift + 1000);
        assert!((speed - 0.1351).abs() < 0.001, "{speed}");
        let path = run(&mut list, lift, lift + 6000);
        assert!(path.windows(2).all(|w| w[1] >= w[0]), "never turns back");
        assert!(!list.moving());
        assert!((list.state(lift + 6000).0 - rest).abs() < THRESHOLD + 0.01);
    }

    #[test]
    fn a_finger_that_stopped_before_lifting_throws_nothing() {
        let mut list = list();
        list.place(100.0);
        let last = flick(&mut list, 250, 150, 0, 100);
        assert!(list.release(last + 150));
        assert_eq!(list.motion, Motion::Still(190.0));
    }

    #[test]
    fn coasting_into_an_end_hands_its_speed_to_the_bounce_spring() {
        let mut list = list();
        list.place(600.0);
        let lift = flick(&mut list, 250, 150, 0, 50); // 2 px/ms
        list.release(lift);
        let path = run(&mut list, lift, lift + 4000);
        let peak = path.iter().cloned().fold(0.0, f32::max);
        assert!(peak > 700.5 && peak < 800.0, "overshoots a little: {peak}");
        assert!(!list.moving());
        assert_eq!(list.offset(lift + 4000), 700, "settles on the end");
        // Critically damped: once past the peak it only falls back.
        let top = path.iter().position(|&p| p == peak).unwrap();
        assert!(path[top..].windows(2).all(|w| w[1] <= w[0] + 0.001));
        assert!(path[top..].iter().all(|&p| p >= 700.0 - THRESHOLD));
    }

    #[test]
    fn let_go_past_an_end_springs_back_without_wobbling() {
        let mut list = list();
        list.press(0, 0);
        list.drag(10, 10);
        list.drag(210, 20);
        let pulled = list.state(20).0;
        assert!(pulled < -50.0);
        list.release(20 + 150);
        let path = run(&mut list, 170, 2000);
        assert!(path.windows(2).all(|w| w[1] >= w[0] - 0.001));
        assert!(path.iter().all(|&p| p <= THRESHOLD));
        assert_eq!(list.offset(2000), 0);
        // Mass 1, stiffness 100: ~95 % home in 0.47 s, as UIKit's bounce.
        let at = |ms: u64| spring_from(pulled, ms);
        assert!(at(470) > pulled * 0.06 && at(470) < pulled * 0.04);
    }

    /// The bounce spring from `from` at rest, `ms` later.
    fn spring_from(from: f32, ms: u64) -> f32 {
        let mut list = list();
        list.motion = Motion::Settle {
            target: 0.0,
            from,
            velocity: 0.0,
            at: 0,
            rate: BOUNCE_RATE,
        };
        list.state(ms).0
    }

    #[test]
    fn a_finger_stops_moving_content_and_that_touch_is_no_tap() {
        let mut list = list();
        list.place(100.0);
        let lift = flick(&mut list, 250, 150, 0, 100);
        list.release(lift);
        list.advance(lift + 200);
        let there = list.offset(lift + 200);
        list.press(100, lift + 200);
        assert!(list.held() && !list.moving());
        assert_eq!(list.offset(lift + 300), there, "it stays under the finger");
        assert!(list.release(lift + 300), "the catch swallows the tap");
        assert!(!list.moving());
        assert_eq!(list.offset(lift + 400), there);
        // A tap on a list at rest is a tap.
        list.press(100, lift + 400);
        assert!(!list.release(lift + 450));
    }

    #[test]
    fn a_reveal_eases_onto_its_target_in_about_a_third_of_a_second() {
        let mut list = list();
        list.reveal(120.0, 0);
        let path = run(&mut list, 0, 1000);
        assert!(
            path.windows(2).all(|w| w[1] >= w[0] - 0.001),
            "no overshoot"
        );
        assert!(path.iter().all(|&p| p <= 120.0 + 0.001));
        assert!(!list.moving());
        // Within half a pixel of it after 400 ms, the frames at 16 ms apart.
        assert!((path[25] - 120.0).abs() < THRESHOLD, "{}", path[25]);
        assert_eq!(list.offset(1000), 120);
        list.reveal(5000.0, 1000);
        assert_eq!(list.rest(), 700.0, "clamped to the content");
    }

    #[test]
    fn content_shorter_than_the_viewport_only_bands() {
        let mut list = Kinetic::new(300);
        list.set_content(200, 0);
        let lift = flick(&mut list, 250, 100, 0, 100);
        list.release(lift);
        run(&mut list, lift, lift + 3000);
        assert_eq!(list.offset(lift + 3000), 0);
    }
}
