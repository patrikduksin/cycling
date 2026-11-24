//! A ride-grid field: a capital title that carries the unit, with an
//! optional symbol before it and the zone tag (`Z4`) at its right, over a
//! big number at the bottom left. A `%` sits on the number's baseline at
//! two-thirds its size. A field filled by its zone or steepness writes in
//! the fill's ink. Its looks keep room for its slot, the widest number it
//! shows at full size; a wider one takes the smaller digits.
use super::{px, specs};
use crate::compose::data::{Data, Effort, Metric, Reads};
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Align;
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::theme::{LAYOUT, Tone};
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct FieldTile;

/// What fills a field behind its words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    None,
    /// The effort's current zone, with its tag beside the title.
    Zone(Effort),
    /// The grade's steepness band.
    Steep,
}

/// How the number reads. A number sheds precision or holds at an end
/// before it outgrows the narrowest box a field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Digits {
    /// As `Metric::write` writes it.
    Metric,
    /// As `Metric::write` writes it, held at this reading at most.
    Upto(i32),
    /// Kilometres from metres with one decimal, up to 999.9.
    Km,
    /// A grade from tenths of a percent: one decimal under 10%, whole
    /// percent from there.
    Grade,
}

#[derive(Debug)]
pub struct Spec {
    /// Capitals naming the metric and its unit, `SPD KM/H`.
    pub title: &'static str,
    pub symbol: Option<Symbol>,
    pub metric: Metric,
    pub digits: Digits,
    pub tone: Tone,
    pub fill: Fill,
    /// The widest number the field shows in the full-size digits.
    pub slot: &'static str,
}

impl Spec {
    pub const fn label(&self) -> &'static str {
        self.title
    }

    fn write(&self, value: Option<i32>, out: &mut Text<12>) {
        let _ = match (self.digits, value) {
            (Digits::Upto(most), Some(value)) => {
                self.metric.write(Some(value.min(most)), out);
                Ok(())
            }
            (Digits::Km, Some(metres)) => {
                let tenths = (metres.clamp(0, 999_949) + 50) / 100;
                write!(out, "{}.{}", tenths / 10, tenths % 10)
            }
            (Digits::Grade, Some(tenths)) if tenths.abs() >= 100 => {
                let percent = (tenths.clamp(-999, 999) + 5 * tenths.signum()) / 10;
                write!(out, "{percent}%")
            }
            _ => {
                self.metric.write(value, out);
                Ok(())
            }
        };
    }

    const fn percent(&self) -> bool {
        matches!(self.slot.as_bytes().last(), Some(b'%'))
    }
}

specs! {
    /// A ride-grid field. Ids: Stats 0–31, Climb 32–63, Workout 64–95.
    pub enum Field: Spec {
        Pwr = 0 => Spec {
            title: "PWR 3S W",
            symbol: Some(Symbol::Power),
            metric: Metric::Power,
            digits: Digits::Metric,
            tone: Tone::Power,
            fill: Fill::Zone(Effort::Power),
            slot: "888",
        },
        Heart = 1 => Spec {
            title: "HEART BPM",
            symbol: Some(Symbol::Heart),
            metric: Metric::Heart,
            digits: Digits::Metric,
            tone: Tone::Heart,
            fill: Fill::Zone(Effort::Heart),
            slot: "188",
        },
        Spd = 2 => Spec {
            title: "SPD KM/H",
            symbol: None,
            metric: Metric::Speed,
            digits: Digits::Metric,
            tone: Tone::Speed,
            fill: Fill::None,
            slot: "88.8",
        },
        Cad = 3 => Spec {
            title: "CAD RPM",
            symbol: None,
            metric: Metric::Cadence,
            digits: Digits::Metric,
            tone: Tone::Cadence,
            fill: Fill::None,
            slot: "188",
        },
        Grade = 4 => Spec {
            title: "GRADE",
            symbol: Some(Symbol::Ramp),
            metric: Metric::Grade,
            digits: Digits::Grade,
            tone: Tone::Grade,
            fill: Fill::None,
            slot: "8.8%",
        },
        Climb = 5 => Spec {
            title: "CLIMB M",
            symbol: Some(Symbol::Peak),
            metric: Metric::Ascent,
            digits: Digits::Upto(9_999),
            tone: Tone::Text,
            fill: Fill::None,
            slot: "888",
        },
        Dist = 6 => Spec {
            title: "DIST KM",
            symbol: None,
            metric: Metric::Distance,
            digits: Digits::Km,
            tone: Tone::Text,
            fill: Fill::None,
            slot: "88.8",
        },
        Np = 7 => Spec {
            title: "NP W",
            symbol: Some(Symbol::Power),
            metric: Metric::NormalizedPower,
            digits: Digits::Metric,
            tone: Tone::Power,
            fill: Fill::None,
            slot: "888",
        },
    }
}

/// One look of a field, best first: each sheds what a narrow box cannot
/// hold, the symbol, then the zone tag (the fill still says the zone), then
/// the full-size digits.
#[derive(Clone, Copy)]
struct Look {
    digits: FaceRole,
    symbol: bool,
    tag: bool,
}

const LOOKS: [Look; 4] = [
    Look {
        digits: LAYOUT.face.digits,
        symbol: true,
        tag: true,
    },
    Look {
        digits: LAYOUT.face.digits,
        symbol: false,
        tag: true,
    },
    Look {
        digits: LAYOUT.face.digits,
        symbol: false,
        tag: false,
    },
    Look {
        digits: LAYOUT.face.digits_small,
        symbol: false,
        tag: false,
    },
];

/// The zone tag the field shows beside its title.
fn tag(zone: u8) -> Text<4> {
    let mut text = Text::new();
    let _ = write!(text, "Z{zone}");
    text
}

/// A number split from its trailing `%`.
fn split(text: &[u8]) -> (&[u8], bool) {
    match text.split_last() {
        Some((b'%', number)) => (number, true),
        _ => (text, false),
    }
}

/// The columns left of the words, and right of them.
fn insets(at: &Scaled) -> (usize, usize) {
    (px(at, LAYOUT.space.gutter), px(at, LAYOUT.space.gap))
}

fn title_width(spec: &Spec, look: Look, at: &Scaled) -> usize {
    let tag_face = at.face(LAYOUT.face.tag);
    let symbol = match spec.symbol.filter(|_| look.symbol) {
        Some(symbol) => at.art.symbol(symbol).ink(0, 0).width() + px(at, LAYOUT.space.tight),
        None => 0,
    };
    let zone = match spec.fill {
        Fill::Zone(_) if look.tag => px(at, LAYOUT.space.tight) + tag_face.fit_width(b"Z7"),
        _ => 0,
    };
    symbol + tag_face.fit_width(spec.title.as_bytes()) + zone
}

/// The room the number takes: its slot in the full-size digits, and in the
/// small digits every number the field can show.
fn number_width(spec: &Spec, look: Look, at: &Scaled) -> usize {
    let small = at.face(LAYOUT.face.digits_small);
    let width = |text: &[u8]| {
        let (number, percent) = split(text);
        let digits = at.face(look.digits);
        digits.fit_width(number) + if percent { small.fit_width(b"%") } else { 0 }
    };
    let slot = width(spec.slot.as_bytes());
    if look.digits != LAYOUT.face.digits_small {
        return slot;
    }
    let (low, high) = spec.metric.ends();
    [None, Some(low), Some(high), spec.metric.turn()]
        .into_iter()
        .map(|value| {
            let mut text = Text::<12>::new();
            spec.write(value, &mut text);
            width(text.as_bytes())
        })
        .fold(slot, usize::max)
}

/// The box `look` of `spec` needs: its width and height.
fn size(spec: &Spec, look: Look, at: &Scaled) -> (usize, usize) {
    let space = &LAYOUT.space;
    let digits = at.face(look.digits);
    let (left, right) = insets(at);
    let inner = title_width(spec, look, at).max(number_width(spec, look, at));
    let height = 2 * px(at, space.pad)
        + at.face(LAYOUT.face.tag).fit_height()
        + px(at, space.gap)
        + digits.fit_height();
    (left + inner + right, height)
}

impl Tile for FieldTile {
    type Arg = Field;
    const NAME: &'static str = "field";

    fn reads(field: Field) -> Option<Reads> {
        Some(Reads::Metric(field.spec().metric))
    }

    /// Every field asks for one box, the room of the widest field's least
    /// look, so fields that share a row or a column share it evenly and a
    /// grid's rules line up. A field takes the best look its box holds.
    fn forms(field: Field, at: &Scaled) -> Forms {
        let spec = field.spec();
        let least = LOOKS[LOOKS.len() - 1];
        let width = Field::ALL
            .iter()
            .map(|field| size(field.spec(), least, at).0)
            .max()
            .unwrap_or(0);
        let shown = |shows: bool| u8::from(shows);
        forms(&[Form {
            width: width as u16,
            height: size(spec, least, at).1 as u16,
            shapes: shown(!matches!(spec.fill, Fill::None)) + shown(spec.symbol.is_some()),
            runs: 2 + shown(matches!(spec.fill, Fill::Zone(_))) + shown(spec.percent()),
        }])
    }

    fn prepare(field: Field, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let spec = field.spec();
        let rect = placed.rect.rect();
        let look = LOOKS
            .into_iter()
            .find(|&look| {
                let (width, height) = size(spec, look, at);
                width <= rect.width() && height <= rect.height()
            })
            .unwrap_or(LOOKS[LOOKS.len() - 1]);
        let theme = data.theme();
        let space = &LAYOUT.space;
        let (zone, filled) = match spec.fill {
            Fill::None => (None, None),
            Fill::Zone(effort) => {
                let zone = data.zone(effort);
                (zone, zone.and_then(|zone| theme.fill(effort, zone)))
            }
            Fill::Steep => (
                None,
                data.metric(spec.metric).map(|tenths| theme.steep(tenths)),
            ),
        };
        let ink = match filled {
            Some((fill, ink)) => {
                out.shape(Shape::Panel {
                    rect,
                    radius: 0,
                    color: fill,
                });
                ink
            }
            None => theme.tone(spec.tone),
        };
        let (left, right) = insets(at);
        let (left, right) = (rect.left + left, rect.right - right);
        let tag_face = at.face(LAYOUT.face.tag);
        let base = rect.top + px(at, space.pad) + tag_face.fit_height();
        let mut x = left;
        if let Some(symbol) = spec.symbol.filter(|_| look.symbol) {
            let icon = at.art.symbol(symbol);
            let ink_box = icon.ink(0, 0);
            let middle = base - tag_face.cap as usize / 2;
            out.shape(Shape::Icon {
                icon: StaticIcon(icon),
                left: x.saturating_sub(ink_box.left),
                top: middle.saturating_sub(ink_box.top + ink_box.height() / 2),
                color: ink,
            });
            x += ink_box.width() + px(at, space.tight);
        }
        let mut title_right = right;
        if let Some(zone) = zone.filter(|_| look.tag && filled.is_some()) {
            let tag = tag(zone);
            let run = out.fit(
                &[tag_face],
                tag.as_bytes(),
                right - x,
                right,
                base,
                Align::Right,
                ink,
            );
            title_right = run.left.saturating_sub(px(at, space.tight));
        }
        let title = spec.title.as_bytes();
        out.fit(
            &[tag_face],
            title,
            title_right.saturating_sub(x),
            x,
            base,
            Align::Left,
            ink,
        );
        number(spec, look, at, data, (left, right, rect.bottom), ink, out);
    }
}

/// The field's number at the bottom left, its `%` beside it. A number wider
/// than the look's slot takes the small digits where the box is too narrow.
fn number(
    spec: &Spec,
    look: Look,
    at: &Scaled,
    data: &dyn Data,
    (left, right, bottom): (usize, usize, usize),
    ink: u16,
    out: &mut Canvas,
) {
    let mut text = Text::<12>::new();
    spec.write(data.metric(spec.metric), &mut text);
    let (number, percent) = split(text.as_bytes());
    let small = at.face(LAYOUT.face.digits_small);
    let percent_width = if percent { small.fit_width(b"%") } else { 0 };
    let base = bottom - px(at, LAYOUT.space.pad);
    let faces = [at.face(look.digits), small];
    let width = (right - left).saturating_sub(percent_width);
    let run = out.fit(&faces, number, width, left, base, Align::Left, ink);
    if percent {
        let after = run.left + run.line.face().ink_width(run.line.text());
        out.fit(
            &[small],
            b"%",
            right.saturating_sub(after),
            after,
            base,
            Align::Left,
            ink,
        );
    }
}
