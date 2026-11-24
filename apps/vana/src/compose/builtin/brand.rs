//! The brand reveal VANA starts with: the mark alone, its spare room even
//! above and below. Any press leaves for the first page.
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::MARK;

const LEAVE: Option<Button> = Some(Button {
    action: Action::Page(OnPage::Back),
    cue: None,
});

pub const BUILTIN: &Builtin = &Builtin {
    tree,
    buttons: Buttons {
        act: LEAVE,
        back: LEAVE,
        move_: LEAVE,
        held: None,
    },
};

fn tree() -> Nodes {
    [
        Node::col(1, Size::grow(1)),
        Node::leaf(MARK, 0, Size::CONTENT),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{Laid, ScreenId};
    use crate::compose::data::{Data, Effort, Metric, PageData};
    use crate::compose::node::Flags;
    use crate::screens::brand::DONE;
    use crate::screens::canvas::Canvas;
    use crate::screens::chrome::{ACCENT, BLACK, Look, Row, Shown, WHITE};
    use crate::screens::{Device, panels};

    struct Reveal(u16);

    impl Data for Reveal {
        fn metric(&self, _: Metric) -> Option<i32> {
            None
        }

        fn zone(&self, _: Effort) -> Option<u8> {
            None
        }

        fn page(&self) -> PageData<'_> {
            PageData::Brand(self.0)
        }
    }

    fn reveal(device: &Device, progress: u16) -> Canvas {
        let laid = Laid::new(ScreenId::Brand, device, Flags::NONE, None).unwrap();
        let mut canvas = Canvas::default();
        laid.prepare(&Reveal(progress), &mut canvas);
        canvas
    }

    fn lit(device: &Device, canvas: &Canvas, color: u16) -> usize {
        let panel = device.scaled.panel;
        (0..panel.height)
            .map(|y| {
                let mut pixels = std::vec![BLACK; panel.width];
                canvas.paint(&mut Row::new(y, &mut pixels));
                for (x, &pixel) in pixels.iter().enumerate() {
                    assert_eq!(
                        pixel,
                        canvas.pixel(Look::PLAIN, x, y, Shown(BLACK)).0,
                        "paint at {x},{y}"
                    );
                }
                pixels.iter().filter(|&&p| p == color).count()
            })
            .sum()
    }

    #[test]
    fn the_reveal_closes_the_ring_round_the_wordmark_in_the_middle() {
        for device in panels::ALL {
            let first = reveal(device, 0);
            assert_eq!(lit(device, &first, ACCENT), 0, "no ring yet");
            assert!(
                lit(device, &first, WHITE) > 50,
                "VANA shows on the first frame"
            );
            let (half, whole) = (reveal(device, DONE / 2), reveal(device, DONE));
            let (halfway, closed) = (lit(device, &half, ACCENT), lit(device, &whole, ACCENT));
            assert!(halfway * 2 > closed * 9 / 10 && halfway * 2 < closed * 11 / 10);
            let what = std::format!("reveal on {:?}", device.scaled.panel);
            let screen = device.scaled.screen();
            whole.assert_laid_out(screen, &["VANA", "LABS"], &what);
            let ring = whole.shapes[0].bounds();
            let (above, below) = (ring.top - screen.top, screen.bottom - ring.bottom);
            assert!(
                above.abs_diff(below) <= 1,
                "{what}: {above} above, {below} below"
            );
        }
    }
}
