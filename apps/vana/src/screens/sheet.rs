use super::canvas::Canvas;
use super::chrome::{BLACK, CARD, PRESSED, Rect, Row, blend, rounded};
use super::cue::Cue;
use super::scaled::Scaled;
use super::sensors::Glance;
use crate::compose::builtin::Laid;
use crate::compose::data::{Data, Effort, ListKind, Metric};
use crate::compose::registry::SPACE;

pub const SENSORS: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Facts {
    pub level: u8,
    pub light: bool,
    pub sensors: heapless::Vec<Glance, SENSORS>,
}

impl Data for Facts {
    fn metric(&self, _: Metric) -> Option<i32> {
        None
    }

    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }

    fn list(&self, _: ListKind) -> u16 {
        0
    }

    fn sheet(&self) -> Option<&Facts> {
        Some(self)
    }
}

const FOOT_UNITS: usize = 18;
const RADIUS_UNITS: usize = 22;
const HANDLE_UNITS: (usize, usize) = (36, 5);
const SHOWN_UNDER: u8 = 8;

#[derive(Clone, Copy, Debug, Hash)]
pub struct Over<'v> {
    pub bottom: usize,
    pub cues: &'v [Cue],
}

pub fn bottom(laid: &Laid, at: &Scaled) -> usize {
    let drawn = laid.tiles().filter(|&(component, _, _)| component != SPACE);
    let last = drawn.map(|(_, _, placed)| placed.rect.bottom);
    let bottom = usize::from(last.max().unwrap_or(0)) + at.unit(FOOT_UNITS);
    bottom.min(at.panel.height)
}

pub const ROOM: (usize, usize) = (13, 3);

pub fn prepare(laid: &Laid, facts: &Facts, bottom: usize, canvas: &mut Canvas) {
    canvas.hide(0..bottom);
    canvas.start_cover(ROOM);
    laid.prepare(facts, canvas);
}

impl Over<'_> {
    pub fn paint(&self, row: &mut Row, canvas: &Canvas, device: &super::Device) {
        let (y, at) = (row.y(), &device.scaled);
        let width = at.panel.width;
        let black = row.ink(BLACK);
        row.each_over(0..width, |_, under| Some(blend(under, black, SHOWN_UNDER)));
        if y < self.bottom {
            let card = self.card(at);
            let fill = row.ink(CARD);
            let solid = y + at.unit(RADIUS_UNITS) < self.bottom;
            if solid {
                row.span_shown(0..width, fill);
            } else {
                let radius = at.unit(RADIUS_UNITS);
                row.each_over(0..width, |x, under| {
                    let coverage = rounded(card, radius, x, y);
                    (coverage > 0).then(|| blend(fill, under, coverage))
                });
            }
            let handle = self.handle(at);
            if (handle.top..handle.bottom).contains(&y) {
                let ink = row.ink(PRESSED);
                let radius = handle.height() / 2;
                row.each_over(handle.left..handle.right, |x, under| {
                    let coverage = rounded(handle, radius, x, y);
                    (coverage > 0).then(|| blend(ink, under, coverage))
                });
            }
            canvas.paint_cover(row);
        }
        for cue in self.cues {
            cue.paint(row, device);
        }
    }

    #[cfg(test)]
    pub fn pixel(
        &self,
        look: super::chrome::Look,
        x: usize,
        y: usize,
        under: super::chrome::Shown,
        canvas: &Canvas,
        device: &super::Device,
    ) -> super::chrome::Shown {
        let at = &device.scaled;
        let mut color = blend(under, look.ink(BLACK), SHOWN_UNDER);
        if y < self.bottom {
            let card = self.card(at);
            let radius = at.unit(RADIUS_UNITS);
            let solid = y + radius < self.bottom;
            let coverage = if solid {
                15
            } else {
                rounded(card, radius, x, y)
            };
            if coverage > 0 {
                color = blend(look.ink(CARD), color, coverage);
            }
            let handle = self.handle(at);
            let coverage = rounded(handle, handle.height() / 2, x, y);
            if coverage > 0 {
                color = blend(look.ink(PRESSED), color, coverage);
            }
            color = canvas.cover_pixel(look, x, y, color);
        }
        for cue in self.cues {
            color = cue.pixel(look, x, y, color, device);
        }
        color
    }

    fn card(&self, at: &Scaled) -> Rect {
        Rect {
            bottom: self.bottom,
            ..at.screen()
        }
    }

    fn handle(&self, at: &Scaled) -> Rect {
        let (width, height) = (at.unit(HANDLE_UNITS.0), at.unit(HANDLE_UNITS.1));
        let left = (at.panel.width - width) / 2;
        let top = self.bottom - at.unit(FOOT_UNITS) / 2 - height / 2;
        Rect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::Rect;
    use crate::screens::workout::{Page, View};
    use crate::sensors::ant::device_type;
    use firmware_shell::shell::Picture;

    fn facts(light: bool) -> Facts {
        Facts {
            level: 60,
            light,
            sensors: [device_type::HEART_RATE, device_type::POWER]
                .map(|kind| Glance::ble(kind, kind == device_type::POWER, None))
                .into_iter()
                .collect(),
        }
    }

    fn pulled(page: Page, device: &'static super::super::Device, light: bool) -> View<'static> {
        let mut view = View::new(page, 0, None);
        view.device = device;
        view.compose_main();
        view.pull_sheet(facts(light));
        view
    }

    #[test]
    fn the_sheet_draws_over_any_screen_and_paints_its_reference() {
        crate::screens::check::enable(true);
        for device in crate::screens::panels::ALL {
            for (page, light) in [(Page::Home, false), (Page::Ride, true)] {
                let mut view = pulled(page, device, light);
                let what = std::format!("{page:?} on {:?}", device.scaled.panel);
                let scene = view.prepare();
                scene.check_layout();
                let width = device.scaled.panel.width;
                for y in 0..device.scaled.panel.height {
                    let mut pixels = std::vec![0x5555; width];
                    scene.paint(&mut Row::new(y, &mut pixels));
                    for (x, &pixel) in pixels.iter().enumerate() {
                        let want = scene.reference_pixel(x, y).0;
                        assert_eq!(pixel, want, "{what} at {x},{y}");
                    }
                }
                let canvas = view.canvas();
                assert!(canvas.shows("SENSORS"), "{what}");
                assert!(canvas.shows(if light { "DAY" } else { "NIGHT" }), "{what}");
                let cues: std::vec::Vec<_> = view.prepare().cues().to_vec();
                assert!(cues.is_empty(), "{what}: the screen's cues go under");
            }
        }
    }

    #[test]
    fn the_sheet_s_tiles_fit_the_room_the_screen_leaves_them() {
        for device in crate::screens::panels::ALL {
            let view = pulled(Page::Home, device, false);
            let laid = view.laids.and_then(|laids| laids.sheet()).unwrap();
            let mut canvas = Canvas::default();
            laid.prepare(&facts(false), &mut canvas);
            canvas.assert_kept("the sheet alone");
            assert!(canvas.shapes.len() <= ROOM.0 && canvas.runs.len() <= ROOM.1);
            let bottom = bottom(laid, &device.scaled);
            let card = Rect {
                left: 0,
                top: 0,
                right: device.scaled.panel.width,
                bottom,
            };
            canvas.assert_laid_out_at(&device.scaled, card, &["SENSORS"], "the sheet");
            assert!(
                bottom < device.scaled.panel.height * 2 / 3,
                "{bottom}: the screen shows under it"
            );
        }
    }
}
