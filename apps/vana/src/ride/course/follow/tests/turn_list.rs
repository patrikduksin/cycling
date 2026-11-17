use super::*;
use crate::ride::course::nav::tests::latches;

pub(super) struct View<'f> {
    pub(super) line: &'f [Crumb],
    pub(super) len: usize,
    pub(super) turns: &'f [Turn],
    pub(super) turn_count: usize,
    pub(super) nav: Navigator,
}

impl<'f> View<'f> {
    pub(super) fn of(follower: &'f Follower) -> Self {
        View {
            line: follower.line,
            len: follower.len,
            turns: follower.turns,
            turn_count: follower.turn_count,
            nav: follower.life.nav().copied().unwrap_or_default(),
        }
    }

    fn listed(&self) -> &'f [Turn] {
        &self.turns[..self.turn_count]
    }
}

pub(super) fn turn_starts_never_decrease(view: &View) -> bool {
    view.listed()
        .windows(2)
        .all(|pair| pair[0].start_mm <= pair[1].start_mm)
}

pub(super) fn each_turn_starts_by_its_end(view: &View) -> bool {
    view.listed()
        .iter()
        .all(|turn| turn.start_mm <= turn.end_mm)
}

pub(super) fn every_end_lies_within_the_line(view: &View) -> bool {
    let length_mm = match view.len {
        0 => 0,
        len => (view.line[len - 1].along * 1000.0) as u32,
    };
    view.listed().iter().all(|turn| turn.end_mm <= length_mm)
}

pub(super) fn latches_index_the_turn_list(view: &View) -> bool {
    let latched = latches(&view.nav);
    [latched.banner, latched.last_sounded, latched.last_beeped]
        .into_iter()
        .flatten()
        .all(|k| k < view.turn_count)
}

/// The one list of the turn-list rules: `$table` builds a table from it.
macro_rules! turn_rules {
    ($table:ident) => {
        $table! {
            "turn starts never decrease" =>
                $crate::ride::course::follow::tests::turn_list::turn_starts_never_decrease,
            "each turn starts by its end" =>
                $crate::ride::course::follow::tests::turn_list::each_turn_starts_by_its_end,
            "every turn ends within the line" =>
                $crate::ride::course::follow::tests::turn_list::every_end_lies_within_the_line,
            "banner, last_sounded and last_beeped index the turn list" =>
                $crate::ride::course::follow::tests::turn_list::latches_index_the_turn_list,
        }
    };
}
pub(super) use turn_rules;

pub(super) type TurnRule = (&'static str, fn(&View) -> bool);

macro_rules! turn_list {
    ($($name:expr => $rule:path),* $(,)?) => {
        pub(super) const TURN_LIST: &[TurnRule] = &[$(($name, $rule)),*];
    };
}
turn_rules!(turn_list);
