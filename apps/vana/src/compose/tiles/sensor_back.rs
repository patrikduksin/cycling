use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Align;
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;

pub struct SensorBack;

const BACK: &[u8] = b"SENSORS";
const WIDEST_CLOCK: &[u8] = b"00:00";

impl Tile for SensorBack {
    type Arg = ();
    const NAME: &'static str = "sensor back";

    fn forms((): (), at: &Scaled) -> Forms {
        let space = &LAYOUT.space;
        let chevron = at.art.symbol(Symbol::Chevron).ink(0, 0).width();
        let words = at.face(LAYOUT.face.tag).fit_width(BACK);
        let clock = at.face(LAYOUT.face.line).fit_width(WIDEST_CLOCK);
        let width = 2 * px(at, space.gutter)
            + chevron
            + px(at, space.tight)
            + words
            + px(at, space.gap)
            + clock;
        forms(&[Form {
            width: width as u16,
            height: px(at, space.top_line) as u16,
            shapes: 1,
            runs: 2,
        }])
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let theme = data.theme();
        let space = &LAYOUT.space;
        let rect = placed.rect.rect();
        let middle = (rect.top + rect.bottom) / 2;
        let left = rect.left + px(at, space.gutter);
        let right = rect.right - px(at, space.gutter);
        let icon = at.art.symbol(Symbol::Chevron);
        let ink = icon.ink(0, 0);
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: left.saturating_sub(ink.left),
            top: middle.saturating_sub(ink.top + ink.height() / 2),
            color: theme.sub,
        });
        let tag = at.face(LAYOUT.face.tag);
        let words = left + ink.width() + px(at, space.tight);
        let clock = at.face(LAYOUT.face.line);
        let shown = out.fit(
            &[clock],
            data.clock().as_bytes(),
            right - words,
            right,
            middle + usize::from(clock.cap) / 2,
            Align::Right,
            theme.text,
        );
        out.fit(
            &[tag],
            BACK,
            shown.left.saturating_sub(words + px(at, space.gap)),
            words,
            middle + usize::from(tag.cap) / 2,
            Align::Left,
            theme.sub,
        );
    }
}
