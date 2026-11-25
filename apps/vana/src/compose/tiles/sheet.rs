use crate::compose::data::Data;
use crate::compose::input::{Action, SheetAct};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect};
use crate::screens::icons::{Icon, Sensor, Size, StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::sheet::{PhoneRow, SENSORS};
use crate::screens::theme::LAYOUT;

const BAR_RADIUS: usize = 14;
const TILE_RADIUS: usize = 18;
const PAD: usize = 10;
const SUN_INSET: usize = 12;
const ICON_GAP: usize = 8;
const WORD_GAP: usize = 2;
const ICON_INK: usize = 16;
const WORD_ROOM: usize = 30;
const DOT: usize = 6;
const STACK_GAP: usize = 5;

fn icon_at(out: &mut Canvas, icon: &'static Icon, (left, top): (usize, usize), color: u16) {
    let ink = icon.ink(0, 0);
    out.shape(Shape::Icon {
        icon: StaticIcon(icon),
        left: left - ink.left,
        top: top - ink.top,
        color,
    });
}

fn icon_centred(
    out: &mut Canvas,
    icon: &'static Icon,
    (left, middle): (usize, usize),
    color: u16,
) -> Rect {
    let ink = icon.ink(0, 0);
    let top = middle - ink.height() / 2;
    icon_at(out, icon, (left, top), color);
    icon.ink(left - ink.left, top - ink.top)
}

pub struct Level;

impl Tile for Level {
    type Arg = ();
    const NAME: &'static str = "level";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(120) as u16,
            height: at.unit(36) as u16,
            shapes: 3,
            runs: 0,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(facts) = data.sheet() else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        let split = rect.left + rect.width() * usize::from(facts.level.min(100)) / 100;
        out.shape(Shape::Split {
            rect,
            radius: at.unit(BAR_RADIUS),
            split,
            colors: [theme.text, theme.raised],
        });
        let middle = (rect.top + rect.bottom) / 2;
        let pad = at.unit(SUN_INSET);
        let small = at.art.symbol(Symbol::SunSmall);
        icon_centred(out, small, (rect.left + pad, middle), theme.bg);
        let large = at.art.symbol(Symbol::SunLarge);
        let width = large.ink(0, 0).width();
        icon_centred(out, large, (rect.right - pad - width, middle), theme.sub);
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Sheet(SheetAct::Level))
    }
}

pub struct Sensors;

impl Tile for Sensors {
    type Arg = ();
    const NAME: &'static str = "sheet sensors";

    fn forms((): (), at: &Scaled) -> Forms {
        let word = at.unit(WORD_ROOM);
        let chevron = at.art.symbol(Symbol::ChevronRight).ink(0, 0).width();
        let icons = SENSORS * at.unit(ICON_INK + ICON_GAP);
        forms(&[Form {
            width: (2 * at.unit(PAD) + icons + at.unit(WORD_GAP) + word + chevron) as u16,
            height: at.unit(40) as u16,
            shapes: 2 + 2 * SENSORS as u8,
            runs: 1,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(facts) = data.sheet() else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: at.unit(TILE_RADIUS),
            color: theme.raised,
        });
        let middle = (rect.top + rect.bottom) / 2;
        let mut x = rect.left + at.unit(PAD);
        for glance in &facts.sensors {
            let icon = at.art.sensor(Sensor::of(glance.kind), Size::Small);
            let (ink, dot) = super::home::sensor_colors(theme, glance);
            let drawn = icon_centred(out, icon, (x, middle), ink);
            out.shape(Shape::Disc {
                cx: (4 * drawn.right) as i32,
                cy: (4 * drawn.top) as i32,
                radius: (2 * at.unit(DOT)) as i32,
                color: dot,
            });
            x = drawn.right + at.unit(ICON_GAP);
        }
        let chevron = at.art.symbol(Symbol::ChevronRight);
        let right = rect.right - at.unit(PAD) - chevron.ink(0, 0).width();
        icon_centred(out, chevron, (right, middle), theme.sub);
        let faces = [at.face(LAYOUT.face.label), at.face(LAYOUT.face.note)];
        let left = x + if facts.sensors.is_empty() {
            0
        } else {
            at.unit(WORD_GAP)
        };
        let base = middle + usize::from(faces[0].cap) / 2;
        let width = right.saturating_sub(left);
        out.fit(
            &faces,
            b"Sensors",
            width,
            left,
            base,
            Align::Left,
            theme.text,
        );
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Sheet(SheetAct::Sensors))
    }
}

pub struct Phone;

impl Tile for Phone {
    type Arg = ();
    const NAME: &'static str = "sheet phone";

    fn forms((): (), at: &Scaled) -> Forms {
        let chevron = at.art.symbol(Symbol::ChevronRight).ink(0, 0).width();
        let (label, note) = (at.face(LAYOUT.face.label), at.face(LAYOUT.face.note));
        let state = note.fit_width(b"Not paired");
        forms(&[Form {
            width: (2 * at.unit(PAD)
                + at.unit(ICON_INK + ICON_GAP)
                + label.fit_width(b"Phone")
                + at.unit(ICON_GAP)
                + state
                + at.unit(ICON_GAP)
                + chevron) as u16,
            height: at.unit(40) as u16,
            shapes: 4,
            runs: 2,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(facts) = data.sheet() else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: at.unit(TILE_RADIUS),
            color: theme.raised,
        });
        let (ink, sub) = if facts.recording {
            (theme.sub, theme.rule)
        } else {
            (theme.text, theme.sub)
        };
        let middle = (rect.top + rect.bottom) / 2;
        let icon = at.art.symbol(Symbol::Phone);
        let drawn = icon_centred(out, icon, (rect.left + at.unit(PAD), middle), ink);
        if facts.phone == PhoneRow::Connected {
            out.shape(Shape::Disc {
                cx: (4 * drawn.right) as i32,
                cy: (4 * drawn.top) as i32,
                radius: (2 * at.unit(DOT)) as i32,
                color: theme.good,
            });
        }
        let chevron = at.art.symbol(Symbol::ChevronRight);
        let right = rect.right - at.unit(PAD) - chevron.ink(0, 0).width();
        icon_centred(out, chevron, (right, middle), sub);
        let (label, note) = (at.face(LAYOUT.face.label), at.face(LAYOUT.face.note));
        let left = drawn.right + at.unit(ICON_GAP);
        let state: &[u8] = match facts.phone {
            PhoneRow::NotPaired => b"Not paired",
            PhoneRow::Paired => b"Paired",
            PhoneRow::Connected => b"",
        };
        let words = out.fit(
            &[note],
            state,
            right.saturating_sub(left + at.unit(ICON_GAP)),
            right - at.unit(ICON_GAP),
            middle + usize::from(note.cap) / 2,
            Align::Right,
            sub,
        );
        out.fit(
            &[label, note],
            b"Phone",
            words.left.saturating_sub(left + at.unit(ICON_GAP)),
            left,
            middle + usize::from(label.cap) / 2,
            Align::Left,
            ink,
        );
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Sheet(SheetAct::Phone))
    }
}

pub struct DayNight;

impl Tile for DayNight {
    type Arg = ();
    const NAME: &'static str = "day night";

    fn forms((): (), at: &Scaled) -> Forms {
        forms(&[Form {
            width: at.unit(40) as u16,
            height: at.unit(40) as u16,
            shapes: 2,
            runs: 1,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(facts) = data.sheet() else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        out.shape(Shape::Panel {
            rect,
            radius: at.unit(TILE_RADIUS),
            color: theme.raised,
        });
        let (symbol, word): (_, &[u8]) = if facts.light {
            (Symbol::Sun, b"Day")
        } else {
            (Symbol::Moon, b"Night")
        };
        let icon = at.art.symbol(symbol);
        let ink = icon.ink(0, 0);
        let note = at.face(LAYOUT.face.note);
        let block = ink.height() + at.unit(STACK_GAP) + usize::from(note.cap);
        let top = rect.top + rect.height().saturating_sub(block) / 2;
        let centre = rect.center_x();
        icon_at(out, icon, (centre - ink.width() / 2, top), theme.text);
        let base = top + block;
        out.fit(
            &[note],
            word,
            rect.width(),
            centre,
            base,
            Align::Center,
            theme.sub,
        );
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::Sheet(SheetAct::DayNight))
    }
}
