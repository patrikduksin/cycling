//! Monotonic inactivity dimming with explicit wake-input consumption.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pub timeout_ms: Option<u64>,
    pub dim_level: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Gate {
    Forward,
    Consume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Idle {
    last_activity: u64,
    held: bool,
    consume_touch: bool,
    dimmed: bool,
}

impl Idle {
    pub const fn new(now: u64) -> Self {
        Self {
            last_activity: now,
            held: false,
            consume_touch: false,
            dimmed: false,
        }
    }

    pub fn tick(&mut self, now: u64, config: Config) {
        self.dimmed = !self.held
            && config
                .timeout_ms
                .is_some_and(|timeout| now.saturating_sub(self.last_activity) >= timeout);
    }

    pub fn contact(&mut self, now: u64) -> Gate {
        let wake = self.dimmed;
        self.last_activity = now;
        self.held = true;
        self.dimmed = false;
        self.consume_touch |= wake;
        if self.consume_touch {
            Gate::Consume
        } else {
            Gate::Forward
        }
    }

    pub fn release(&mut self, now: u64) -> Gate {
        if self.held {
            self.last_activity = now;
        }
        self.held = false;
        if core::mem::take(&mut self.consume_touch) {
            Gate::Consume
        } else {
            Gate::Forward
        }
    }

    pub fn button(&mut self, now: u64) -> Gate {
        let wake = self.dimmed;
        self.last_activity = now;
        self.dimmed = false;
        if wake { Gate::Consume } else { Gate::Forward }
    }

    pub const fn dimmed(self) -> bool {
        self.dimmed
    }

    pub fn age_ms(self, now: u64) -> u64 {
        now.saturating_sub(self.last_activity)
    }

    pub fn effective(self, selected: u8, config: Config) -> u8 {
        if self.dimmed {
            selected.min(config.dim_level)
        } else {
            selected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: Config = Config {
        timeout_ms: Some(1_000),
        dim_level: 10,
    };

    #[test]
    fn exact_deadline_disabled_and_dim_above_selected() {
        let mut idle = Idle::new(100);
        idle.tick(1_099, CONFIG);
        assert!(!idle.dimmed());
        idle.tick(1_100, CONFIG);
        assert!(idle.dimmed());
        assert_eq!(idle.effective(5, CONFIG), 5);
        idle.tick(
            10_000,
            Config {
                timeout_ms: None,
                ..CONFIG
            },
        );
        assert!(!idle.dimmed());
    }

    #[test]
    fn held_contact_stays_awake_and_wake_gesture_is_consumed_through_release() {
        let mut idle = Idle::new(0);
        idle.tick(1_000, CONFIG);
        assert_eq!(idle.contact(1_000), Gate::Consume);
        idle.tick(5_000, CONFIG);
        assert!(!idle.dimmed());
        assert_eq!(idle.contact(5_000), Gate::Consume);
        assert_eq!(idle.release(5_000), Gate::Consume);
        idle.tick(5_999, CONFIG);
        assert!(!idle.dimmed());
        idle.tick(6_000, CONFIG);
        assert!(idle.dimmed());
        assert_eq!(idle.button(6_000), Gate::Consume);
        assert_eq!(idle.contact(6_100), Gate::Forward);
        assert_eq!(idle.release(6_200), Gate::Forward);
    }

    #[test]
    fn first_wake_button_is_consumed_and_next_is_forwarded() {
        let mut idle = Idle::new(0);
        idle.tick(1_000, CONFIG);
        assert_eq!(idle.button(1_000), Gate::Consume);
        assert_eq!(idle.button(1_001), Gate::Forward);
    }
}
