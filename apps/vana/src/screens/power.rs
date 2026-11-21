//! Full-screen power sheets: confirm turning off, the idle warning, the
//! farewell before the device turns off, and the charge shown while the
//! device rests on USB power with VANA off.
use super::canvas::Canvas;
use super::chrome::{BLACK, Battery, Row};
use super::cue::Cue;
use crate::compose::builtin::{Laid, ScreenId};
use crate::compose::data::{Count, Data, Effort, ListKind, Metric};

/// The idle warning's ring is full at this many seconds.
pub const WARNING_SECONDS: u64 = crate::compose::tiles::power::WARNING_SECONDS as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// "Turn off?" after BACK on Home: ACT turns off, anything else returns.
    /// With `unsaved` rides the phone has not confirmed it says to sync them
    /// first, and ACT does not turn off.
    Confirm { unsaved: u8 },
    /// Idle auto-off countdown; any button keeps VANA on.
    Warning { seconds: u64 },
    /// The farewell, `progress` thousandths of the way to black.
    TurningOff { progress: u16 },
    /// VANA is off and the device charges on USB power; `known` is
    /// whether the battery has reported its charge yet.
    Charging { battery: Battery, known: bool },
}

impl Sheet {
    /// Whether the sheet moves from frame to frame.
    pub const fn animated(self) -> bool {
        match self {
            Self::Confirm { .. } => false,
            Self::Warning { .. } | Self::TurningOff { .. } => true,
            Self::Charging { battery, .. } => battery.charging && battery.percent < 100,
        }
    }

    pub const fn screen(self) -> ScreenId {
        ScreenId::power(self)
    }

    /// The sheet laid out for one frame from its screen's tiles.
    pub fn canvas(self, laid: &Laid) -> Canvas {
        let mut canvas = Canvas::default();
        laid.prepare(&self, &mut canvas);
        canvas
    }

    /// Paint one row of the sheet from its `canvas`, and the cues of the
    /// buttons that answer it.
    pub fn paint(canvas: &Canvas, cues: &[Cue], device: &super::Device, row: &mut Row) {
        row.fill(BLACK);
        canvas.paint(row);
        for cue in cues {
            cue.paint(row, device);
        }
    }

    pub fn cues(laid: &Laid) -> heapless::Vec<Cue, 3> {
        laid.controls().shown(laid.focus()).collect()
    }
}

impl Data for Sheet {
    fn metric(&self, _: Metric) -> Option<i32> {
        None
    }

    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }

    fn list(&self, _: ListKind) -> u16 {
        0
    }

    fn count(&self, count: Count) -> Option<i32> {
        match (*self, count) {
            (Self::Confirm { unsaved }, Count::Unsaved) => Some(i32::from(unsaved)),
            (Self::Warning { seconds }, Count::Seconds) => Some(seconds.min(999) as i32),
            (Self::TurningOff { progress }, Count::Farewell) => Some(i32::from(progress)),
            (Self::Charging { battery, known }, Count::Charge) => {
                known.then_some(i32::from(battery.percent.min(100)))
            }
            (Self::Charging { battery, .. }, Count::ChargeFill) => Some(i32::from(battery.fill())),
            (Self::Charging { battery, .. }, Count::Charging) => Some(i32::from(battery.charging)),
            _ => None,
        }
    }
}

#[cfg(any(test, feature = "invariants"))]
impl Sheet {
    /// Fail a sheet whose runs or shapes leave the panel, or whose runs
    /// overlap.
    pub fn check_layout(canvas: &Canvas, device: &super::Device) {
        if !super::check::enabled() {
            return;
        }
        canvas.assert_kept(format_args!("a power sheet on {:?}", device.scaled));
        let panel = device.scaled.screen();
        let breaks = super::check::canvas_breaks(canvas, panel, &[("screen", panel)], 0, &[]);
        assert!(
            breaks.is_empty(),
            "a power sheet on {:?} breaks its layout:\n{}",
            device.scaled,
            breaks.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::Device;
    use crate::screens::brand;
    use crate::screens::canvas::Shape;
    use crate::screens::chrome::{AMBER, GREEN, Look, Shown, WHITE, mix};
    use crate::screens::cue::Icon;
    use crate::screens::{C606, panels};

    fn laid(sheet: Sheet, device: &Device) -> Laid {
        Laid::new(
            sheet.screen(),
            device,
            Default::default(),
            Default::default(),
            None,
        )
        .expect("every sheet has a tree")
    }

    fn canvas(sheet: Sheet, device: &Device) -> Canvas {
        sheet.canvas(&laid(sheet, device))
    }

    fn assert_paints_reference(sheet: Sheet, device: &Device) {
        let canvas = canvas(sheet, device);
        let panel = device.scaled.panel;
        for look in [false, true].map(crate::screens::look::look) {
            for y in 0..panel.height {
                let mut pixels = std::vec![0x5555; panel.width];
                Sheet::paint(
                    &canvas,
                    &[],
                    device,
                    &mut Row::new(y, &mut pixels).looked(look),
                );
                for (x, &pixel) in pixels.iter().enumerate() {
                    let want = canvas.pixel(look, x, y, look.ink(BLACK));
                    assert_eq!(Shown(pixel), want, "{sheet:?} at {x},{y}");
                }
            }
        }
    }

    fn charge(percent: u8, charging: bool, phase: u8) -> Sheet {
        Sheet::Charging {
            battery: Battery {
                percent,
                charging,
                phase,
            },
            known: true,
        }
    }

    fn lit(canvas: &Canvas, color: u16, columns: core::ops::Range<usize>) -> usize {
        (0..320)
            .flat_map(|y| columns.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.pixel(Look::PLAIN, x, y, Shown(BLACK)).0 == color)
            .count()
    }

    fn count(sheet: Sheet, color: u16) -> usize {
        lit(&canvas(sheet, &C606), color, 0..240)
    }

    #[test]
    fn sheets_lay_out_whole_and_paint_their_reference() {
        let sheets = [
            (
                Sheet::Confirm { unsaved: 0 },
                &["TURN OFF", "RIDES ARE SAVED"][..],
            ),
            (
                Sheet::Confirm { unsaved: 1 },
                &["TURN OFF", "SYNC RIDE FIRST"][..],
            ),
            (
                Sheet::Confirm { unsaved: 3 },
                &["TURN OFF", "SYNC RIDES FIRST"][..],
            ),
            (
                Sheet::Warning { seconds: 9 },
                &[
                    "TURNING OFF IN",
                    "9",
                    "SECONDS",
                    "PRESS ANY BUTTON",
                    "TO STAY ON",
                ],
            ),
            (Sheet::Warning { seconds: 60 }, &["60"]),
            (Sheet::Warning { seconds: 1000 }, &["99"]),
            (Sheet::TurningOff { progress: 0 }, &["TURNING OFF", "VANA"]),
            (Sheet::TurningOff { progress: 500 }, &["VANA"]),
            (charge(64, true, 2), &["64%", "CHARGING"]),
            (charge(100, true, 0), &["100%", "CHARGED"]),
            (charge(3, false, 0), &["3%", "ON USB POWER"]),
            (
                Sheet::Charging {
                    battery: Battery::default(),
                    known: false,
                },
                &["--", "ON USB POWER"],
            ),
        ];
        crate::screens::check::enable(true);
        for device in panels::ALL {
            for (sheet, want) in sheets {
                let canvas = canvas(sheet, device);
                let what = std::format!("{sheet:?} on {:?}", device.scaled.panel);
                canvas.assert_laid_out(device.scaled.screen(), want, &what);
                Sheet::check_layout(&canvas, device);
                assert_paints_reference(sheet, device);
            }
        }
    }

    /// Shutdown follows the last farewell frame, so it must be black.
    #[test]
    fn the_last_farewell_frame_is_black() {
        for device in panels::ALL {
            let canvas = canvas(
                Sheet::TurningOff {
                    progress: brand::DONE,
                },
                device,
            );
            let panel = device.scaled.panel;
            let dark = (0..panel.height).all(|y| {
                (0..panel.width).all(|x| canvas.pixel(Look::PLAIN, x, y, Shown(BLACK)).0 == BLACK)
            });
            assert!(dark, "{panel:?}");
        }
    }

    #[test]
    fn turn_off_answers_with_corner_cues_and_draws_the_power_symbol() {
        for device in panels::ALL {
            let cues = |sheet| {
                let laid = laid(sheet, device);
                let focus = laid.focus();
                let shown = laid.controls().shown(focus).map(|cue| cue.icon);
                shown.collect::<std::vec::Vec<_>>()
            };
            let answers = cues(Sheet::Confirm { unsaved: 0 });
            assert_eq!(answers.len(), 2);
            assert!(answers.contains(&Icon::Back) && answers.contains(&Icon::Check));
            assert_eq!(
                cues(Sheet::Confirm { unsaved: 2 }),
                [Icon::Back],
                "unsaved rides take away the way to turn off"
            );
            let confirm = canvas(Sheet::Confirm { unsaved: 0 }, device);
            assert_eq!(confirm.find("TURN OFF").map(|run| run.color), Some(WHITE));
            let ring = confirm.shapes.iter().any(|shape| {
                matches!(shape, Shape::Ring { ring, color: WHITE, .. } if ring.sweep < crate::screens::chrome::TURN)
            });
            let bar = confirm
                .shapes
                .iter()
                .any(|shape| matches!(shape, Shape::Bar { color: WHITE, .. }));
            assert!(
                ring && bar,
                "the ring open at the top, the bar through its gap"
            );
        }
    }

    #[test]
    fn the_warning_ring_empties_with_the_seconds() {
        let amber = |seconds| count(Sheet::Warning { seconds }, AMBER);
        let (full, half) = (amber(30), amber(15));
        assert!(
            half * 2 > full * 8 / 10 && half * 2 < full * 12 / 10,
            "{half} of {full}"
        );
    }

    #[test]
    fn charging_climbs_from_the_charge_to_full_and_rests_when_charged() {
        let climb = mix(GREEN, BLACK, 8);
        assert!(
            count(charge(40, true, 0), climb) < 100,
            "the climb starts at the charge"
        );
        assert!(count(charge(40, true, 3), climb) > 1000, "and reaches full");
        assert!(charge(40, true, 1).animated());
        assert!(!charge(100, true, 1).animated());
        assert!(!charge(40, false, 1).animated());
        // A quarter's charge lights the ring from 12 o'clock to 3: the right
        // half only.
        let canvas = canvas(charge(25, false, 0), &C606);
        assert!(lit(&canvas, GREEN, 120..240) > 500);
        assert!(
            lit(&canvas, GREEN, 0..120) < 100,
            "only the arc's round start"
        );
    }
}
