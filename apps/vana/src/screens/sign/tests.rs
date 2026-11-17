use super::*;
use crate::screens::chrome::{BLACK, HEIGHT, Shown, WIDTH};
use std::vec::Vec;

fn turn(glyph: Glyph, metres: u32) -> Sign {
    Sign::Turn {
        glyph,
        distance: Distance::from_mm(metres * 1000),
    }
}

fn chip(glyph: Glyph, metres: u32) -> Sign {
    Sign::Chip {
        glyph,
        distance: Distance::from_mm(metres * 1000),
    }
}

fn off(metres: u32) -> Sign {
    Sign::OffCourse {
        back: Distance::from_mm(metres * 1000),
    }
}

fn width(sign: Sign, form: SignForm) -> usize {
    place(sign, form, false).unwrap().rect().width()
}

#[test]
fn fitted_signs_are_as_wide_as_their_mockups() {
    assert_eq!(width(turn(Glyph::Right, 250), SignForm::Badge), 137);
    assert_eq!(width(turn(Glyph::Right, 40), SignForm::Badge), 118);
    assert_eq!(width(off(140), SignForm::Badge), 152);
    assert_eq!(width(chip(Glyph::Right, 6_400), SignForm::Chip), 97);
}

#[test]
fn each_form_sits_where_its_mockup_draws_it() {
    let rect = |sign, form, radar| place(sign, form, radar).unwrap().rect();
    let banner = rect(turn(Glyph::Left, 120), SignForm::Banner, false);
    assert_eq!(
        (banner.left, banner.top, banner.right, banner.bottom),
        (0, 22, 240, 104)
    );
    let paired = rect(off(140), SignForm::Banner, true);
    assert_eq!(
        (paired.left, paired.right),
        (28, 240),
        "the radar keeps its strip"
    );
    let badge = rect(turn(Glyph::Right, 250), SignForm::Badge, false);
    assert_eq!((badge.left, badge.top, badge.bottom), (8, 30, 90));
    let chip = rect(chip(Glyph::Right, 6_400), SignForm::Chip, false);
    assert_eq!((chip.left, chip.top, chip.bottom), (24, 72, 100));
}

#[test]
fn a_paired_radar_moves_the_content_by_half_its_strip() {
    for sign in [turn(Glyph::Left, 120), off(140), Sign::Arrived] {
        let alone = place(sign, SignForm::Banner, false).unwrap();
        let paired = place(sign, SignForm::Banner, true).unwrap();
        assert_eq!(paired.glyph_at.0, alone.glyph_at.0 + RADAR_SHIFT);
        let lefts = |placed: &Placed| placed.lines.iter().map(|l| l.left).collect::<Vec<_>>();
        let shifted: Vec<_> = lefts(&alone).iter().map(|x| x + RADAR_SHIFT).collect();
        assert_eq!(lefts(&paired), shifted);
    }
}

#[test]
fn beside_a_radar_the_badge_keeps_8_px_from_the_map_s_edge() {
    for sign in [turn(Glyph::Right, 250), off(140), Sign::Arrived] {
        let alone = place(sign, SignForm::Badge, false).unwrap();
        let paired = place(sign, SignForm::Badge, true).unwrap();
        assert_eq!(paired.rect().left, RADAR + 8, "{sign:?}");
        assert_eq!(
            paired.rect().right - paired.rect().left,
            alone.rect().right - alone.rect().left,
            "{sign:?}"
        );
        assert_eq!(paired.glyph_at.0, alone.glyph_at.0 + RADAR, "{sign:?}");
    }
}

#[test]
fn the_chip_only_shows_the_next_turn_at_rest() {
    assert!(place(turn(Glyph::Right, 120), SignForm::Chip, false).is_none());
    assert!(place(off(140), SignForm::Chip, false).is_none());
    assert!(place(Sign::Arrived, SignForm::Chip, false).is_none());
    assert!(place(Sign::None, SignForm::Banner, false).is_none());
}

#[test]
fn the_data_pages_show_a_banner_and_only_main_the_chip() {
    let at_rest = chip(Glyph::Right, 900);
    let banner = turn(Glyph::Right, 120);
    assert_eq!(form(RidePage::Main, at_rest), Some(SignForm::Chip));
    for page in [RidePage::Main, RidePage::Stats, RidePage::Climb] {
        assert_eq!(form(page, banner), Some(SignForm::Banner), "{page:?}");
        assert_eq!(form(page, off(80)), Some(SignForm::Banner), "{page:?}");
        assert_eq!(
            form(page, Sign::Arrived),
            Some(SignForm::Banner),
            "{page:?}"
        );
        assert_eq!(form(page, Sign::None), None, "{page:?}");
    }
    for page in [RidePage::Stats, RidePage::Climb, RidePage::Lap] {
        assert_eq!(form(page, at_rest), None, "{page:?}");
    }
    assert_eq!(form(RidePage::Lap, banner), None);
    for sign in [at_rest, banner, off(80), Sign::Arrived, Sign::TurnAround] {
        assert_eq!(form(RidePage::Map, sign), Some(SignForm::Badge), "{sign:?}");
    }
    assert_eq!(form(RidePage::Map, Sign::None), None);
}

#[test]
fn painted_signs_match_the_per_pixel_reference() {
    let signs = [
        chip(Glyph::Right, 6_400),
        chip(Glyph::SharpLeft, 90),
        turn(Glyph::Left, 120),
        turn(Glyph::UTurn, 20),
        turn(Glyph::Finish, 12_000),
        off(140),
        Sign::Arrived,
        Sign::TurnAround,
    ];
    let under = |x: usize, y: usize| if (x + y) % 7 < 3 { RAISED } else { BLACK };
    for sign in signs {
        for form in [SignForm::Banner, SignForm::Badge, SignForm::Chip] {
            for radar in [false, true] {
                let Some(placed) = place(sign, form, radar) else {
                    continue;
                };
                for y in 0..HEIGHT {
                    let mut pixels = [0u16; WIDTH];
                    for (x, pixel) in pixels.iter_mut().enumerate() {
                        *pixel = under(x, y);
                    }
                    placed.paint(&mut Row::new(y, &mut pixels));
                    for (x, &painted) in pixels.iter().enumerate() {
                        let under = Shown(under(x, y));
                        let want = placed.pixel(x, y, under).unwrap_or(under).0;
                        assert_eq!(
                            painted, want,
                            "{sign:?} {form:?} radar {radar} at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn turn_around_is_an_amber_u_turn_whose_words_stay_inside_it() {
    for page in [RidePage::Main, RidePage::Stats, RidePage::Climb] {
        assert_eq!(
            form(page, Sign::TurnAround),
            Some(SignForm::Banner),
            "{page:?}"
        );
    }
    assert!(place(Sign::TurnAround, SignForm::Chip, false).is_none());
    for form in [SignForm::Banner, SignForm::Badge] {
        for radar in [false, true] {
            let placed = place(Sign::TurnAround, form, radar).unwrap();
            let bounds = placed.rect();
            assert!(bounds.right <= WIDTH, "{form:?} radar {radar}: {bounds:?}");
            let inked =
                |x: usize, y: usize| placed.pixel(x, y, Shown(BLACK)) != Some(Shown(OFF_FILL));
            let edge = bounds.right - 8;
            let words_left = bounds.left + 50;
            for y in bounds.top + 4..bounds.bottom - 4 {
                for x in edge..bounds.right - BADGE_RADIUS {
                    assert!(!inked(x, y), "{form:?} radar {radar}: ink at ({x}, {y})");
                }
            }
            let rows: Vec<usize> = (bounds.top..bounds.bottom)
                .filter(|&y| (words_left..edge).any(|x| inked(x, y)))
                .collect();
            assert!(rows.len() > 30, "{form:?}: two lines of words, {rows:?}");
        }
    }
}
