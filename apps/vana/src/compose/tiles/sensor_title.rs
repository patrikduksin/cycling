use crate::compose::data::Data;
use crate::compose::kit::px;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile, forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Align;
use crate::screens::icons::{StaticIcon, Symbol};
use crate::screens::scaled::Scaled;
use crate::screens::sensors::{List, MenuMessage, widest};
use crate::screens::theme::LAYOUT;

pub struct SensorTitle;

fn columns(at: &Scaled) -> (usize, usize) {
    let space = &LAYOUT.space;
    let chevron = at.art.symbol(Symbol::Chevron).ink(0, 0);
    let left = px(at, space.gutter);
    (left, left + chevron.width() + px(at, space.gutter))
}

impl Tile for SensorTitle {
    type Arg = List;
    const NAME: &'static str = "sensor title";

    fn forms(list: List, at: &Scaled) -> Forms {
        let heading = at.face(LAYOUT.face.heading);
        let title = match list {
            List::Sensors => {
                let trouble = MenuMessage::ALL
                    .into_iter()
                    .filter(|message| message.on_sensors())
                    .chain(MenuMessage::NOTICES)
                    .map(|message| message.short().as_bytes());
                let label = at.face(LAYOUT.face.label);
                widest::of(label, trouble).max(heading.fit_width(b"SENSORS"))
            }
            List::Nearby => heading.fit_width(b"NEARBY"),
        };
        forms(&[Form {
            width: (columns(at).1 + title + px(at, LAYOUT.space.gutter)) as u16,
            height: (heading.fit_height() + px(at, LAYOUT.space.pad)) as u16,
            shapes: 1,
            runs: 1,
        }])
    }

    fn prepare(list: List, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(menu) = data.sensors() else {
            return;
        };
        let theme = data.theme();
        let rect = placed.rect.rect();
        let heading = at.face(LAYOUT.face.heading);
        let base = rect.top + heading.fit_height();
        let (chevron_at, title_at) = columns(at);
        let icon = at.art.symbol(Symbol::Chevron);
        let ink = icon.ink(0, 0);
        let cap_middle = base - usize::from(heading.cap) / 2;
        out.shape(Shape::Icon {
            icon: StaticIcon(icon),
            left: (rect.left + chevron_at).saturating_sub(ink.left),
            top: cap_middle.saturating_sub(ink.top + ink.height() / 2),
            color: theme.sub,
        });
        let (title, tone) = menu.title(list);
        let left = rect.left + title_at;
        out.fit(
            &[heading, at.face(LAYOUT.face.label)],
            title.as_bytes(),
            rect.right.saturating_sub(left),
            left,
            base,
            Align::Left,
            theme.tone(tone),
        );
    }
}
