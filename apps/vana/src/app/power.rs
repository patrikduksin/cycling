//! Turning VANA off: the rider's confirm sheet, automatic off when idle and
//! the hand-over to the device's coordinated shutdown.
//!
//! The device owns hardware sequencing (`PowerControl`), the shell owns the
//! input inactivity clock, and VANA owns when turning off is safe: never while
//! a ride is recording, paused or has a pending write.
use super::Runtime;
use crate::screens::power::Sheet as View;
use device_api::observation::Observation;
use device_api::power::{Operation, PowerControl, State};
use firmware_shell::shell::Shell;

/// No input, sensor data, movement or USB power for this long turns VANA off.
pub const AUTO_OFF_MS: u64 = 15 * 60_000;
/// The idle sheet warns this long before turning off; any button keeps VANA on.
pub const AUTO_OFF_WARNING_MS: u64 = 30_000;
/// An unanswered "Turn off?" sheet closes by itself.
const CONFIRM_TIMEOUT_MS: u64 = 8_000;
/// How long a refused or failed turn-off stays on Home.
const NOTICE_MS: u64 = 4_000;
/// Failed submissions of the TURNING OFF frame before turning off anyway: a
/// panel that rejects frames cannot show it, and staying on drains the battery.
const TURNING_OFF_ATTEMPTS: u8 = 3;
/// GPS movement beyond this (about 30 m, in 1e-7 degrees) counts as activity.
const MOVED_E7: i32 = 2_700;

/// The farewell runs this long before VANA asks the device to turn off.
pub const FAREWELL_MS: u64 = 480;

pub(super) const TURNING_OFF: Sheet = Sheet::TurningOff {
    started: None,
    progress: 0,
    drawn: false,
    failed: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sheet {
    Closed,
    /// The rider asked to turn off and has not answered yet.
    Confirm {
        since: u64,
    },
    /// The farewell is on its way to the panel; the request follows once
    /// its last, black frame is drawn. `progress` is in thousandths of
    /// `FAREWELL_MS` from the first frame, `started`.
    TurningOff {
        started: Option<u64>,
        progress: u16,
        drawn: bool,
        failed: u8,
    },
    /// The device accepted shutdown; VANA stops drawing until power returns.
    Requested {
        sequence: u32,
    },
}

/// Why VANA did not turn off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Notice {
    /// The device refused the request (for example while its companion starts).
    Refused,
    /// The device accepted, then recovered without turning off.
    Failed,
    /// A preference edit (profile or saved sensors) could not be written;
    /// turning off again discards it.
    Unsaved,
}

impl Notice {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::Refused => "CANNOT TURN OFF",
            Self::Failed => "TURN OFF FAILED",
            Self::Unsaved => "CHANGES NOT SAVED",
        }
    }
}

/// What the idle policy says right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoOff {
    Awake,
    /// Seconds left before turning off, shown on the idle sheet.
    Warning {
        seconds: u64,
    },
    Due,
}

/// Everything the idle policy needs, gathered by its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Idle {
    /// A ride is recording or paused, or a ride write is pending.
    pub busy: bool,
    pub usb_powered: bool,
    /// Since the last button, touch or reported activity (shell clock).
    pub input_idle_ms: u64,
    /// Since the last sensor reading or GPS movement (VANA clock).
    pub quiet_ms: u64,
}

/// Whether two fixes (1e-7 degrees) are more than about 30 m apart. Longitude
/// wraps at ±180°, and the arithmetic is wide enough for any pair of fixes.
pub(super) fn moved(here: (i32, i32), anchor: (i32, i32)) -> bool {
    const TURN_E7: i64 = 3_600_000_000;
    let latitude = (i64::from(here.0) - i64::from(anchor.0)).abs();
    let longitude = (i64::from(here.1) - i64::from(anchor.1)).rem_euclid(TURN_E7);
    let longitude = longitude.min(TURN_E7 - longitude);
    latitude + longitude > i64::from(MOVED_E7)
}

/// Turn off only an idle, unplugged device with no ride to protect.
pub fn auto_off(idle: Idle) -> AutoOff {
    if idle.busy || idle.usb_powered {
        return AutoOff::Awake;
    }
    let idle_ms = idle.input_idle_ms.min(idle.quiet_ms);
    if idle_ms >= AUTO_OFF_MS {
        AutoOff::Due
    } else if idle_ms >= AUTO_OFF_MS - AUTO_OFF_WARNING_MS {
        AutoOff::Warning {
            seconds: (AUTO_OFF_MS - idle_ms).div_ceil(1000),
        }
    } else {
        AutoOff::Awake
    }
}

pub(super) struct Power {
    pub(super) sheet: Sheet,
    pub(super) auto: AutoOff,
    notice: Option<(Notice, u64)>,
    usb_powered: bool,
    last_activity_ms: u64,
    anchor_e7: Option<(i32, i32)>,
    /// The unsaved edit the rider was told about; turning off again discards
    /// that edit only. A later edit is reported again.
    unsaved_warned: Option<u32>,
    pub(super) standby: Standby,
}

impl Default for Power {
    fn default() -> Self {
        Self {
            sheet: Sheet::Closed,
            auto: AutoOff::Awake,
            notice: None,
            usb_powered: false,
            last_activity_ms: 0,
            anchor_e7: None,
            unsaved_warned: None,
            standby: Standby::default(),
        }
    }
}

impl Power {
    /// Note sensor data, GPS movement and USB power from this tick's sample.
    /// `live_sensors` is fresh data the sample omits: BLE outside a ride, ANT
    /// radar and any connected ANT channel still receiving.
    pub(super) fn observe(
        &mut self,
        now: u64,
        sample: &crate::ride::log::Sample,
        live_sensors: bool,
        power_status: Observation<u8>,
    ) {
        // Stock treats companion power status zero as USB charging.
        self.usb_powered = matches!(
            power_status,
            Observation::Fresh { value: 0, .. } | Observation::Stale { value: 0, .. }
        );
        let sensing = live_sensors
            || sample.speed_mm_s.is_some_and(|v| v > 0)
            || sample.power_watts.is_some()
            || sample.heart_bpm.is_some()
            || sample.cadence_tenths.is_some();
        let moved = match (sample.location_e7, self.anchor_e7) {
            (Some(here), Some(anchor)) => moved(here, anchor),
            (Some(_), None) => true,
            _ => false,
        };
        if moved {
            self.anchor_e7 = sample.location_e7;
        }
        // USB power counts as activity, so unplugging starts a full idle
        // period with its warning rather than turning off at once.
        if sensing || moved || self.usb_powered {
            self.last_activity_ms = now;
        }
    }

    pub(super) fn notice(&self, now: u64) -> Option<Notice> {
        self.notice
            .filter(|(_, at)| now.saturating_sub(*at) < NOTICE_MS)
            .map(|(notice, _)| notice)
    }

    /// The sheet VANA draws instead of the current page, if any.
    pub(super) fn view(&self) -> Option<View> {
        match self.sheet {
            Sheet::Confirm { .. } => Some(View::Confirm),
            Sheet::TurningOff { progress, .. } => Some(View::TurningOff { progress }),
            Sheet::Requested { .. } => Some(View::TurningOff {
                progress: crate::screens::brand::DONE,
            }),
            Sheet::Closed => match self.auto {
                AutoOff::Warning { seconds } => Some(View::Warning { seconds }),
                AutoOff::Due => Some(View::TurningOff { progress: 0 }),
                AutoOff::Awake => None,
            },
        }
    }
}

impl Power {
    /// Move the farewell on to `now`, before its frame is drawn.
    pub(super) fn animate(&mut self, now: u64) {
        if let Sheet::TurningOff {
            started, progress, ..
        } = &mut self.sheet
        {
            let since = *started.get_or_insert(now);
            let elapsed = now.saturating_sub(since).min(FAREWELL_MS);
            *progress = (elapsed * u64::from(crate::screens::brand::DONE) / FAREWELL_MS) as u16;
        }
    }
}

impl Runtime {
    /// Ask to turn off, as TopLeft (BACK) on Home does. Refused mid-ride.
    pub(super) fn open_power_sheet(&mut self, now: u64) {
        if !self.recording() {
            self.power.sheet = Sheet::Confirm { since: now };
        }
    }

    /// Buttons while a power sheet is shown. Returns whether the sheet took them.
    pub(super) fn power_input(&mut self, button: device_api::input::Button) -> bool {
        use device_api::input::Button;
        match self.power.sheet {
            Sheet::Confirm { .. } => {
                self.power.sheet = if button == Button::BottomRight && !self.recording() {
                    TURNING_OFF
                } else {
                    Sheet::Closed
                };
                true
            }
            Sheet::TurningOff { .. } | Sheet::Requested { .. } => true,
            // The idle warning closes once the shell sees this press as activity.
            Sheet::Closed => matches!(self.power.auto, AutoOff::Warning { .. } | AutoOff::Due),
        }
    }

    /// A sheet frame was submitted; `ok` is whether the panel accepted it.
    /// The farewell is done once its last frame is on the panel; a rejected
    /// frame is drawn again before shutdown proceeds.
    pub(super) fn power_drawn(&mut self, ok: bool) {
        // A frame VANA could not draw at all (another owner has the panel)
        // counts as a failed attempt, so shutdown is still bounded.
        if let Sheet::TurningOff {
            drawn,
            failed,
            progress,
            ..
        } = &mut self.power.sheet
        {
            if ok {
                *drawn |= *progress >= crate::screens::brand::DONE;
            } else {
                *failed = failed.saturating_add(1);
                *drawn = *failed >= TURNING_OFF_ATTEMPTS;
                log::warn!(target: "sdk.power", "turning-off frame failed attempt={}", failed);
                self.display.redraw();
            }
        }
    }

    /// Whether the device is turning off at VANA's request; nothing else draws.
    pub fn turning_off(&self) -> bool {
        matches!(self.power.sheet, Sheet::Requested { .. })
    }

    /// Advance the turn-off flow and idle policy, and hand an accepted
    /// "turn off" to the device once its frame is on the panel. Composition
    /// calls this each pass while the device reports power work ready.
    pub fn power<D, I, P, B>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        control: &mut impl PowerControl,
        now: u64,
    ) where
        D: device_api::display::Display,
        I: device_api::input::InputSource,
        P: device_api::power::Power,
        B: device_api::storage::OwnedFlash,
    {
        let busy = self.recording();
        let before = self.power.view();
        if self.preferences.unsaved_edit().is_none() {
            self.power.unsaved_warned = None;
        }
        if let Sheet::Requested { sequence } = self.power.sheet {
            let status = control.status();
            if status.ready && status.sequence == sequence {
                // The device recovered instead of turning off.
                if status.state == State::Failed || status.state == State::Completed {
                    self.power.sheet = Sheet::Closed;
                    self.power.notice = Some((Notice::Failed, now));
                    system.activity(now);
                }
            }
        }
        if let Sheet::Confirm { since } = self.power.sheet
            && (busy || now.saturating_sub(since) >= CONFIRM_TIMEOUT_MS)
        {
            self.power.sheet = Sheet::Closed;
        }
        self.power.auto = if self.power.sheet == Sheet::Closed {
            auto_off(Idle {
                busy,
                usb_powered: self.power.usb_powered,
                input_idle_ms: system.inactive_ms(now),
                quiet_ms: now.saturating_sub(self.power.last_activity_ms),
            })
        } else {
            AutoOff::Awake
        };
        if self.power.auto == AutoOff::Due {
            self.power.sheet = TURNING_OFF;
            self.power.auto = AutoOff::Awake;
        }
        if busy && matches!(self.power.sheet, Sheet::TurningOff { .. }) {
            self.power.sheet = Sheet::Closed;
        }
        // VANA cannot draw while the terminal owns the panel or it is blank:
        // count each pass as a failed frame, so turning off stays bounded.
        if !self.display.is_active() || system.foreground == firmware_shell::shell::Screen::Blank {
            self.power_drawn(false);
        }
        // Save a profile edit still settling first. If the write fails, say
        // so once and stay on; turning off again then discards the edit.
        if matches!(self.power.sheet, Sheet::TurningOff { drawn: true, .. })
            && !self.flush_preferences(system, now)
            && self.power.unsaved_warned != self.preferences.unsaved_edit()
        {
            log::warn!(target: "sdk.power", "preferences not saved; staying on");
            self.power.unsaved_warned = self.preferences.unsaved_edit();
            self.power.sheet = Sheet::Closed;
            self.power.notice = Some((Notice::Unsaved, now));
            system.activity(now);
        }
        if matches!(self.power.sheet, Sheet::TurningOff { drawn: true, .. }) {
            match control.request(Operation::Shutdown) {
                Ok(()) => {
                    log::info!(target: "sdk.power", "shutdown accepted");
                    self.power.sheet = Sheet::Requested {
                        sequence: control.status().sequence,
                    };
                }
                Err(error) => {
                    log::warn!(target: "sdk.power", "shutdown refused error={:?}", error);
                    self.power.sheet = Sheet::Closed;
                    self.power.notice = Some((Notice::Refused, now));
                    system.activity(now);
                }
            }
        }
        if self.power.view() != before {
            self.display.redraw();
        }
    }
}

/// While VANA is off on USB power the charge shows for this long, then the
/// screen goes dark until the rider turns VANA on.
pub const STANDBY_LIT_MS: u64 = 20_000;
/// Charging standby frames: often enough for the charging animation.
const STANDBY_FRAME_MS: u64 = 500;

/// Charging standby as VANA draws it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Standby {
    /// When standby began; `None` while VANA runs.
    since: Option<u64>,
    next_frame_ms: u64,
    /// The dark frame after the lit period is on the panel.
    dark: bool,
}

impl Runtime {
    /// Draw charging standby: the device rests on USB power with VANA off
    /// (`device_api::power::State::Charging`), and nothing else runs.
    /// Composition calls this each pass in that state and keeps the
    /// backlight on while it returns `true`; afterwards the screen is dark.
    pub fn standby<D, I, P, B>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        input: &impl device_api::input::InputObservation,
        now: u64,
    ) -> bool
    where
        D: device_api::display::Display,
        I: device_api::input::InputSource,
        P: device_api::power::Power,
        B: device_api::storage::OwnedFlash,
    {
        let standby = &mut self.power.standby;
        let since = *standby.since.get_or_insert(now);
        let lit = now.saturating_sub(since) < STANDBY_LIT_MS;
        if !lit {
            if !standby.dark {
                system.draw_canvas(240, 320, |row| row.fill(crate::screens::chrome::BLACK));
                standby.dark = !system.display_error;
            }
            return false;
        }
        if now < standby.next_frame_ms {
            return true;
        }
        standby.next_frame_ms = now.saturating_add(STANDBY_FRAME_MS);
        let snapshot = input.snapshot(now);
        let percent = snapshot.and_then(|s| match s.battery {
            Observation::Fresh { value, .. } | Observation::Stale { value, .. } => Some(value.0),
            Observation::Unavailable => None,
        });
        let charging = snapshot.is_some_and(|s| {
            matches!(
                s.power,
                Observation::Fresh { value: 0, .. } | Observation::Stale { value: 0, .. }
            )
        });
        let sheet = View::Charging {
            battery: crate::screens::chrome::Battery {
                percent: percent.unwrap_or(0),
                charging,
                phase: (now / STANDBY_FRAME_MS % u64::from(crate::screens::chrome::CHARGE_STEPS))
                    as u8,
            },
            known: percent.is_some(),
        };
        let canvas = sheet.canvas();
        let light = self.light();
        system.draw_canvas(240, 320, |row| {
            View::paint(&canvas, row);
            if light {
                crate::screens::look::apply(row.pixels_mut());
            }
        });
        true
    }
}
