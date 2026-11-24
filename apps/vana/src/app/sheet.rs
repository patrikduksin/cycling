use super::Runtime;
use super::preferences::Theme;
use crate::compose::input::{self, Action, Focus, SheetAct};
use crate::compose::registry::LEVEL;
use crate::screens::sheet::{self, Facts, SENSORS};
use crate::screens::touch::{Control, Hit, Pull};
use device_api::ant::Ant;
use device_api::input::Point;

const STEP: u8 = 10;
const LOWEST: u8 = 10;
const HIGHEST: u8 = 100;
const PULL_EDGE_UNITS: usize = 24;

impl Runtime {
    pub(super) fn sheet_open(&self) -> bool {
        self.composition.laids().sheet().is_some()
    }

    pub(super) fn open_sheet(&mut self) {
        self.composition.laids_mut().open_sheet(self.device);
    }

    pub(super) fn close_sheet(&mut self) {
        self.composition.laids_mut().close_sheet();
    }

    pub(super) fn pulls_down(&self, y: usize) -> bool {
        y < self.device.scaled.unit(PULL_EDGE_UNITS)
    }

    pub(super) fn sheet_hit(&self, point: Point) -> Hit {
        let close = Control::Composed(Action::Sheet(SheetAct::Close), Focus::Screen);
        let control = self.composition.laids().sheet().and_then(|laid| {
            let (x, y) = (usize::from(point.x), usize::from(point.y));
            let mut cues = laid.controls().shown(laid.focus());
            if let Some(cue) = cues.find(|cue| cue.target(self.device).contains(x, y)) {
                return Some(Control::Key(cue.role));
            }
            if y >= sheet::bottom(laid, &self.device.scaled) {
                return Some(close);
            }
            let (focus, action) = input::hit(laid.buttons(), &laid.solved(), point.x, point.y)?;
            Some(match action {
                Action::Sheet(SheetAct::Level) => {
                    let bar = laid.box_of(LEVEL)?;
                    Control::Level {
                        left: bar.left as u16,
                        right: bar.right as u16,
                    }
                }
                _ => Control::Composed(action, focus),
            })
        });
        Hit {
            control,
            swipes: false,
            pull: Some(Pull::Up),
        }
    }

    pub(super) fn sheet_act(&mut self, act: SheetAct, ant: &mut impl Ant, now: u64) {
        let level = self.preferences.display.brightness;
        match act {
            SheetAct::Close => self.close_sheet(),
            SheetAct::Dimmer => self.set_level(level.saturating_sub(STEP), now),
            SheetAct::Brighter => self.set_level(level.saturating_add(STEP), now),
            SheetAct::Level => {}
            SheetAct::DayNight => {
                let mut options = self.preferences.options;
                options.theme = if self.light() {
                    Theme::Night
                } else {
                    Theme::Day
                };
                self.set_options(options, now);
            }
            SheetAct::Sensors => {
                self.close_sheet();
                self.open_sensors(ant, now);
            }
        }
    }

    pub(super) fn drag_level(&mut self, (left, right): (u16, u16), x: u16, now: u64) {
        let width = u32::from(right.saturating_sub(left)).max(1);
        let along = u32::from(x.clamp(left, right) - left);
        let steps = u32::from(HIGHEST / STEP);
        let step = (along * steps + width / 2) / width;
        self.set_level(step as u8 * STEP, now);
    }

    fn set_level(&mut self, level: u8, now: u64) {
        let level = level.clamp(LOWEST, HIGHEST);
        if self.preferences.display.brightness != level {
            let mut display = self.preferences.display;
            display.brightness = level;
            self.set_display(display, now);
        }
    }

    pub(super) fn sheet_facts(&self, now: u64) -> Option<Facts> {
        self.sheet_open().then(|| Facts {
            level: self.preferences.display.brightness,
            light: self.light(),
            sensors: self.glance(now).iter().take(SENSORS).copied().collect(),
        })
    }
}
