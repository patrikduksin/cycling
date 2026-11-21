use super::*;
use crate::screens::chrome::{BLACK, Shown};
use crate::screens::panels::ALL;
use crate::screens::workout::{Page, View};
use crate::screens::{BASE, Layout, ride::ClimbLent};
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

fn signs() -> [Sign; 10] {
    [
        chip(Glyph::Right, 6_400),
        chip(Glyph::SharpLeft, 990),
        chip(Glyph::Finish, 999_000),
        turn(Glyph::Left, 120),
        turn(Glyph::UTurn, 990),
        turn(Glyph::Finish, 999_000),
        off(140),
        off(999_000),
        Sign::Arrived,
        Sign::TurnAround,
    ]
}

const FORMS: [SignForm; 3] = [SignForm::Banner, SignForm::Badge, SignForm::Chip];

/// The chip's box on the main page of `layout`, on a course.
fn main_chip(radar: bool, layout: &Layout) -> Rect {
    let flags = crate::screens::ride::main_flags(true, radar, false, false);
    let main = crate::screens::ride::main_laid(layout, Default::default(), flags);
    crate::screens::ride::main_chip(&main).unwrap()
}

fn placed(sign: Sign, form: SignForm, radar: bool, layout: &Layout) -> Option<Placed> {
    match form {
        SignForm::Chip => place_chip(sign, main_chip(radar, layout), layout),
        _ => place(sign, form, radar, layout),
    }
}

fn on_c606(sign: Sign, form: SignForm, radar: bool) -> Option<Placed> {
    placed(sign, form, radar, &BASE)
}

fn width(sign: Sign, form: SignForm) -> usize {
    on_c606(sign, form, false).unwrap().rect().width()
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
    let rect = |sign, form, radar| on_c606(sign, form, radar).unwrap().rect();
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
    assert_eq!(
        (chip.left, chip.top, chip.bottom),
        (10, 71, 99),
        "where the tree puts it"
    );
}

#[test]
fn a_paired_radar_moves_the_content_by_half_its_strip() {
    for layout in ALL {
        let shift = layout.ride.centre_shift;
        for sign in [turn(Glyph::Left, 120), off(140), Sign::Arrived] {
            let alone = place(sign, SignForm::Banner, false, layout).unwrap();
            let paired = place(sign, SignForm::Banner, true, layout).unwrap();
            assert_eq!(paired.glyph_at.0, alone.glyph_at.0 + shift);
            let lefts = |placed: &Placed| placed.lines.iter().map(|l| l.left).collect::<Vec<_>>();
            let shifted: Vec<_> = lefts(&alone).iter().map(|x| x + shift).collect();
            assert_eq!(lefts(&paired), shifted, "{:?}", layout.scaled);
            assert_eq!(
                paired.rect().left,
                layout.ride.body.left,
                "beside the strip"
            );
        }
    }
}

#[test]
fn beside_a_radar_the_badge_keeps_8_units_from_the_map_s_edge() {
    for layout in ALL {
        let map = layout.ride.body.left;
        let what = |sign| std::format!("{sign:?} on {:?}", layout.scaled);
        for sign in [turn(Glyph::Right, 250), off(140), Sign::Arrived] {
            let alone = place(sign, SignForm::Badge, false, layout).unwrap();
            let paired = place(sign, SignForm::Badge, true, layout).unwrap();
            let inset = layout.scaled.unit(8);
            assert_eq!(alone.rect().left, inset, "{}", what(sign));
            assert_eq!(paired.rect().left, map + inset, "{}", what(sign));
            assert_eq!(
                paired.rect().width(),
                alone.rect().width(),
                "{}",
                what(sign)
            );
            assert_eq!(paired.glyph_at.0, alone.glyph_at.0 + map, "{}", what(sign));
        }
    }
}

#[test]
fn every_sign_s_ink_stays_inside_it_on_every_panel() {
    for layout in ALL {
        for radar in [false, true] {
            for form in FORMS {
                for sign in signs() {
                    let Some(placed) = placed(sign, form, radar, layout) else {
                        continue;
                    };
                    let what =
                        std::format!("{sign:?} {form:?} radar {radar} on {:?}", layout.scaled);
                    let (bounds, spot) = (placed.rect(), placed.spot);
                    assert!(
                        contains(spot.fill, bounds),
                        "{what}: {bounds:?} leaves {spot:?}"
                    );
                    let (left, top) = placed.glyph_at;
                    let glyph = placed.glyph.ink(left, top);
                    let fill = placed.fill_rect().0;
                    assert!(
                        contains(fill, glyph),
                        "{what}: glyph {glyph:?} leaves {fill:?}"
                    );
                    for label in &placed.lines {
                        let ink = Rect {
                            left: label.left,
                            top: label.top,
                            right: label.columns().end,
                            bottom: label.bottom(),
                        };
                        assert!(contains(bounds, ink), "{what}: {ink:?} leaves {bounds:?}");
                    }
                }
            }
        }
    }
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

#[test]
fn the_chip_only_shows_the_next_turn_at_rest() {
    assert!(on_c606(turn(Glyph::Right, 120), SignForm::Chip, false).is_none());
    assert!(on_c606(off(140), SignForm::Chip, false).is_none());
    assert!(on_c606(Sign::Arrived, SignForm::Chip, false).is_none());
    assert!(on_c606(Sign::None, SignForm::Banner, false).is_none());
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
    let under = |x: usize, y: usize| if (x + y) % 7 < 3 { RAISED } else { BLACK };
    for layout in ALL {
        let panel = layout.scaled.panel;
        for sign in signs() {
            for form in FORMS {
                for radar in [false, true] {
                    let Some(placed) = placed(sign, form, radar, layout) else {
                        continue;
                    };
                    for y in 0..panel.height {
                        let mut pixels: Vec<u16> = (0..panel.width).map(|x| under(x, y)).collect();
                        placed.paint(&mut Row::new(y, &mut pixels));
                        for (x, &painted) in pixels.iter().enumerate() {
                            let under = Shown(under(x, y));
                            let want = placed.pixel(x, y, under).unwrap_or(under).0;
                            assert_eq!(
                                painted, want,
                                "{sign:?} {form:?} radar {radar} on {:?} at ({x}, {y})",
                                layout.scaled
                            );
                        }
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
    assert!(on_c606(Sign::TurnAround, SignForm::Chip, false).is_none());
    for form in [SignForm::Banner, SignForm::Badge] {
        for radar in [false, true] {
            let placed = on_c606(Sign::TurnAround, form, radar).unwrap();
            let bounds = placed.rect();
            assert!(
                bounds.right <= chrome::WIDTH,
                "{form:?} radar {radar}: {bounds:?}"
            );
            let inked =
                |x: usize, y: usize| placed.pixel(x, y, Shown(BLACK)) != Some(Shown(OFF_FILL));
            let edge = bounds.right - 8;
            let words_left = bounds.left + 50;
            for y in bounds.top + 4..bounds.bottom - 4 {
                for x in edge..bounds.right - placed.spot.radius {
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

#[test]
fn every_ride_page_keeps_its_layout_under_a_sign_on_every_panel() {
    for layout in ALL {
        for radar in [false, true] {
            for page in [
                RidePage::Main,
                RidePage::Stats,
                RidePage::Climb,
                RidePage::Map,
            ] {
                for sign in signs() {
                    let mut view = View::new(Page::Ride, 0, None);
                    view.layout = layout;
                    view.ride_page = page;
                    view.course = true;
                    view.nav = sign;
                    view.radar_paired = radar;
                    match page {
                        RidePage::Climb => view.show_climb(ClimbLent::NONE),
                        RidePage::Map => view.show_map(None),
                        _ => {}
                    }
                    view.prepare().check_layout();
                }
            }
        }
    }
}
