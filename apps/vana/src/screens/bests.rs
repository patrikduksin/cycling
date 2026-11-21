//! BESTS: the rider's all-time bests, one line each with its value and the
//! day it was set. Opened from Settings, PROFILE; BACK returns there. ACT asks
//! CLEAR BESTS?, which ACT (check cue) confirms and BACK (back cue) cancels.
//! Where bests cannot be kept, the page says so instead of listing them.
//!
//! Power over 5 s, 1, 5 and 20 min comes first in `POWER`, then the longest
//! ride by distance and by time and the most ascent. A best not set yet
//! shows `--` in `TERTIARY`, and a best set before the clock was known shows
//! no day.
use crate::kept::bests::Mark;

/// What the page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    List,
    /// ACT asked to clear them.
    Confirm,
    /// There is no place to keep bests.
    Unavailable,
    /// The file cannot be read; ACT asks to start over.
    Damaged,
    /// The file is of another version; only FORMAT replaces it.
    Foreign,
    /// The store would not take the bests; they are in memory only.
    NotSaved,
}

impl Mode {
    pub const fn asks(self) -> crate::compose::builtin::Asks {
        use crate::compose::builtin::Asks;
        match self {
            Self::List => Asks::Own,
            Self::Confirm => Asks::Question,
            _ => Asks::Note,
        }
    }

    /// The note shown in place of the list, as its place in `NOTES`.
    pub const fn note(self) -> Option<u8> {
        match self {
            Self::Unavailable => Some(0),
            Self::Damaged => Some(1),
            Self::Foreign => Some(2),
            Self::NotSaved => Some(3),
            Self::List | Self::Confirm => None,
        }
    }
}

/// Why the bests do not show, and what follows from it.
pub const NOTES: [[&str; 2]; 4] = [
    ["BESTS NOT KEPT", "NO PLACE TO KEEP THEM"],
    ["BESTS FILE DAMAGED", "IT CANNOT BE READ"],
    ["BESTS NOT FORMATTED", "OFF AND ON TO FORMAT"],
    ["BESTS NOT SAVED", "STORAGE REFUSED THEM"],
];

/// A best by its place on the page: the power bests, then the longest ride,
/// the longest time and the most ascent.
pub fn mark(bests: &crate::kept::bests::Bests, index: u8) -> Mark {
    match index {
        0..4 => bests.power[usize::from(index)],
        4 => bests.distance_m,
        5 => bests.time_s,
        _ => bests.ascent_m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kept::bests::Bests;
    use crate::screens::chrome::Rect;
    use crate::screens::workout::{Page, View};
    use crate::screens::{Layout, panels};
    use firmware_shell::rendering::font::FaceRole;

    fn shown(layout: &'static Layout, bests: Bests, mode: Mode) -> View<'static> {
        let mut view = View::new(Page::Bests, 0, None);
        view.layout = layout;
        view.show_bests(bests, mode);
        view.compose_main();
        view.prepare();
        view
    }

    fn view(bests: Bests) -> View<'static> {
        shown(&crate::screens::BASE, bests, Mode::List)
    }

    fn page(layout: &Layout) -> Rect {
        layout.chrome.content
    }

    fn mark(value: u32, day: u16) -> Mark {
        Mark { value, day }
    }

    #[test]
    fn bests_list_each_value_with_its_day() {
        let bests = Bests {
            power: [
                mark(1999, 20_000),
                mark(450, 20_001),
                mark(312, 20_020),
                mark(255, 20_021),
            ],
            distance_m: mark(123_400, 20_022),
            time_s: mark(5 * 3600 + 20 * 60, 20_023),
            ascent_m: mark(1_850, 20_024),
        };
        let view = view(bests);
        assert!(view.cues.is_empty());
        let want = [
            "BESTS",
            "5 S",
            "20 MIN",
            "1999 W",
            "312 W",
            "123.4 KM",
            "5:20:00",
            "1850 M",
            "OCT 4 2024",
            "OCT 28 2024",
        ];
        let page = page(&crate::screens::BASE);
        view.canvas().assert_laid_out(page, &want, "bests");
    }

    #[test]
    fn power_bests_show_watts_per_kilogram_only_once_the_weight_is_set() {
        let bests = Bests {
            power: [
                mark(1999, 20_000),
                mark(450, 20_001),
                mark(312, 20_020),
                Mark::default(),
            ],
            ..Bests::default()
        };
        let mut view = View::new(Page::Bests, 0, None);
        view.show_bests(bests, Mode::List);
        view.compose_main();
        view.prepare();
        assert!(!view.canvas().shows("W/KG"));
        view.rider = crate::ride::rider::Rider::default().step_weight(1, true);
        view.prepare();
        view.canvas().assert_laid_out(
            page(&crate::screens::BASE),
            &["28.6 W/KG", "6.4 W/KG", "4.5 W/KG", "1999 W", "312 W"],
            "bests with weight",
        );
    }

    #[test]
    fn clearing_asks_first_and_missing_storage_says_so() {
        let full = page(&crate::screens::BASE);
        let confirm = shown(&crate::screens::BASE, Bests::default(), Mode::Confirm);
        confirm.canvas().assert_laid_out(
            full,
            &["CLEAR BESTS?", "THEY CANNOT COME BACK"],
            "confirm",
        );
        assert_eq!(confirm.cues.len(), 2);
        crate::screens::check::enable(true);
        for layout in panels::ALL {
            let mut confirm = shown(layout, Bests::default(), Mode::Confirm);
            let scene = confirm.prepare();
            assert!(
                scene.bar.is_none(),
                "{:?}: the bar would cut the back cue",
                layout.scaled
            );
            scene.check_layout();
        }
        let none = shown(&crate::screens::BASE, Bests::default(), Mode::Unavailable);
        none.canvas()
            .assert_laid_out(full, &["BESTS NOT KEPT", "NO PLACE TO KEEP THEM"], "none");
        assert!(none.cues.is_empty());
    }

    /// A 20 minute best under 10 W/kg at the default weight, whose W/kg keeps
    /// a word's gap from "20 MIN". From 10.0 W/kg, which no rider holds for
    /// 20 minutes, it comes closer than a word on the C606.
    const TWENTY_MINUTES_UNDER_10_W_KG: u32 = 650;

    #[test]
    fn the_longest_bests_and_every_message_lay_out_whole() {
        let widest = Bests {
            power: [
                mark(1999, 20_000),
                mark(1999, 20_000),
                mark(999, 20_000),
                mark(TWENTY_MINUTES_UNDER_10_W_KG, 20_000),
            ],
            distance_m: mark(9_999_900, 20_022),
            time_s: mark(99 * 3600 + 59 * 60 + 59, 20_023),
            ascent_m: mark(99_999, 20_024),
        };
        let messages = [
            (Mode::Confirm, ["CLEAR BESTS?", "THEY CANNOT COME BACK"]),
            (
                Mode::Unavailable,
                ["BESTS NOT KEPT", "NO PLACE TO KEEP THEM"],
            ),
            (Mode::Damaged, ["BESTS FILE DAMAGED", "IT CANNOT BE READ"]),
            (
                Mode::Foreign,
                ["BESTS NOT FORMATTED", "OFF AND ON TO FORMAT"],
            ),
            (Mode::NotSaved, ["BESTS NOT SAVED", "STORAGE REFUSED THEM"]),
        ];
        crate::screens::check::enable(true);
        for layout in panels::ALL {
            let what = std::format!("{:?}", layout.scaled);
            let mut view = shown(layout, widest, Mode::List);
            view.prepare().check_layout();
            let want = ["BESTS", "9999.9 KM", "99:59:59", "MOST ASCENT", "99999 M"];
            view.canvas().assert_laid_out(page(layout), &want, &what);
            view.rider = crate::ride::rider::Rider::default().step_weight(1, true);
            view.prepare().check_layout();
            let canvas = view.canvas();
            assert!(canvas.shows("28.6 W/KG"), "{what}");
            let tenths = view.rider.watts_per_kg_tenths(TWENTY_MINUTES_UNDER_10_W_KG);
            let tenths = tenths.unwrap();
            assert!(
                tenths < 100,
                "{what}: 20 minutes at {tenths} tenths of a W/kg"
            );
            let per_kg = std::format!("{}.{} W/KG", tenths / 10, tenths % 10);
            let rect = |text: &str| {
                let run = canvas.find(text).unwrap();
                crate::screens::chrome::layout::text(run).unwrap().1
            };
            let word = layout.scaled.face(FaceRole::Caption).glyph(b' ').advance;
            let gap = rect(&per_kg).left - rect("20 MIN").right;
            assert!(gap >= usize::from(word), "{what}: {per_kg} meets 20 MIN");
            for (mode, want) in messages {
                let mut view = shown(layout, Bests::default(), mode);
                view.prepare().check_layout();
                view.canvas().assert_laid_out(page(layout), &want, &what);
            }
        }
    }

    #[test]
    fn a_best_not_set_yet_says_so_and_a_missing_day_shows_none() {
        let empty = view(Bests::default());
        assert!(empty.canvas().shows("--"));
        assert!(!empty.canvas().shows(" W"));
        let undated = view(Bests {
            power: [
                mark(300, 0),
                Mark::default(),
                Mark::default(),
                Mark::default(),
            ],
            ..Bests::default()
        });
        assert!(undated.canvas().shows("300 W"));
        assert!(!undated.canvas().shows("1970"));
    }
}
