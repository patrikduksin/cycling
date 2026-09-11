//! Exact canvas change tracking for deciding whether the LCD needs an update.

#[derive(Default)]
pub struct Tracker {
    drawn: bool,
}

impl Tracker {
    pub fn changed<T: PartialEq>(&self, canvas: &[T], displayed: &[T]) -> bool {
        !self.drawn || canvas != displayed
    }

    pub fn commit<T: Copy>(&mut self, canvas: &[T], displayed: &mut [T]) {
        displayed.copy_from_slice(canvas);
        self.drawn = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_and_changed_frames_draw_while_identical_frames_skip() {
        let mut tracker = Tracker::default();
        let mut displayed = [0; 4];
        let first = [0; 4];
        assert!(tracker.changed(&first, &displayed));
        tracker.commit(&first, &mut displayed);
        assert!(!tracker.changed(&first, &displayed));

        let changed = [0, 1, 0, 0];
        assert!(tracker.changed(&changed, &displayed));
        tracker.commit(&changed, &mut displayed);
        assert!(!tracker.changed(&changed, &displayed));
    }
}
