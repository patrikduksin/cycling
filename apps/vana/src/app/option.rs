//! `OPTION` reports VANA's ride, display and sound options as `key=value`;
//! `OPTION NAME VALUE` sets one, with NAME the key in capitals.
use super::Runtime;
use super::mode::DisplayMode;
use super::preferences::{Options, Theme, Wheel};
use crate::clock::Zone;
use crate::ride::alerts::Settings;
use crate::ride::laps::AutoLap;
use crate::ride::wheel_size::PLAUSIBLE_MM;
use core::fmt::Write;

/// The backlight percents `OPTION BRIGHTNESS` takes, in steps of
/// `BRIGHTNESS_STEP`.
const BRIGHTNESS: core::ops::RangeInclusive<u8> = 10..=100;
const BRIGHTNESS_STEP: u8 = 10;

enum Change {
    Options(Options),
    Brightness(u8),
}

impl Runtime {
    pub(super) fn option_command(
        &mut self,
        name: Option<&str>,
        value: Option<&str>,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        match (name, value) {
            (None, None) => {}
            (Some(name), Some(value)) => {
                let Some(change) = change(self.preferences.options, name, value) else {
                    return "INVALID";
                };
                match change {
                    Change::Options(options) => self.set_options(options, now),
                    Change::Brightness(percent) => {
                        let mut display = self.preferences.display;
                        display.brightness = percent;
                        self.set_display(display, now);
                    }
                }
                self.display.redraw();
            }
            _ => return "INVALID",
        }
        if self.report_options(output).is_err() {
            return "OUTPUT";
        }
        "OK"
    }

    fn report_options(&self, output: &mut impl Write) -> core::fmt::Result {
        let options = &self.preferences.options;
        let alerts = Settings::decode(&options.alerts).unwrap_or_default();
        let on = |flag: bool| if flag { "on" } else { "off" };
        write!(
            output,
            "auto_pause={} sounds={} auto_lap=",
            on(options.auto_pause),
            on(options.sounds)
        )?;
        match options.auto_lap {
            AutoLap::Off => write!(output, "off")?,
            AutoLap::Every(km) => write!(output, "{km}")?,
            AutoLap::AtStart => write!(output, "start")?,
        }
        write!(
            output,
            " wheel={} wheel_mode={} hr_low={} hr_high={} power_low={} power_high={} cadence_low={} fuel={} radar={} theme={} display=",
            options.wheel.size_mm(),
            if options.wheel.manual {
                "manual"
            } else {
                "auto"
            },
            alerts.heart_low_bpm,
            alerts.heart_high_bpm,
            alerts.power_low_w,
            alerts.power_high_w,
            alerts.cadence_low_rpm,
            alerts.fuel_every_min,
            on(alerts.radar),
            match options.theme {
                Theme::Day => "day",
                Theme::Night => "night",
            },
        )?;
        for letter in options.mode.policy().name.chars() {
            output.write_char(letter.to_ascii_lowercase())?;
        }
        write!(
            output,
            " brightness={} zone=",
            self.preferences.display.brightness
        )?;
        match options.zone {
            Zone::Chile => write!(output, "chile")?,
            Zone::Fixed(quarters) => {
                let sign = if quarters < 0 { '-' } else { '+' };
                let quarters = quarters.unsigned_abs();
                write!(output, "utc{sign}{}", quarters / 4)?;
                if quarters % 4 != 0 {
                    write!(output, ":{}", quarters % 4 * 15)?;
                }
            }
        }
        write!(
            output,
            " stored={} persistence={}",
            self.preferences.stored.name(),
            self.preferences.persistence().name()
        )
    }
}

fn change(mut options: Options, name: &str, value: &str) -> Option<Change> {
    let flag = || match value {
        "ON" => Some(true),
        "OFF" => Some(false),
        _ => None,
    };
    let mut alerts = Settings::decode(&options.alerts).unwrap_or_default();
    match name {
        "AUTO_PAUSE" => options.auto_pause = flag()?,
        "SOUNDS" => options.sounds = flag()?,
        "AUTO_LAP" => {
            options.auto_lap = match value {
                "OFF" => AutoLap::Off,
                "START" => AutoLap::AtStart,
                km => match km.parse().ok()? {
                    km @ 1..=super::preferences::AUTO_LAP_MAX_KM => AutoLap::Every(km),
                    _ => return None,
                },
            }
        }
        "WHEEL" => {
            let mm = value.parse().ok().filter(|mm| PLAUSIBLE_MM.contains(mm))?;
            options.wheel = Wheel {
                mm: Some(mm),
                manual: true,
            };
        }
        "WHEEL_MODE" => {
            options.wheel.manual = match value {
                "AUTO" => false,
                "MANUAL" => true,
                _ => return None,
            }
        }
        "HR_LOW" => alerts.heart_low_bpm = value.parse().ok()?,
        "HR_HIGH" => alerts.heart_high_bpm = value.parse().ok()?,
        "POWER_LOW" => alerts.power_low_w = value.parse().ok()?,
        "POWER_HIGH" => alerts.power_high_w = value.parse().ok()?,
        "CADENCE_LOW" => alerts.cadence_low_rpm = value.parse().ok()?,
        "FUEL" => alerts.fuel_every_min = value.parse().ok()?,
        "RADAR" => alerts.radar = flag()?,
        "DISPLAY" => {
            options.mode = (0..)
                .map_while(DisplayMode::from_byte)
                .find(|mode| mode.policy().name == value)?;
        }
        "THEME" => {
            options.theme = match value {
                "DAY" => Theme::Day,
                "NIGHT" => Theme::Night,
                _ => return None,
            }
        }
        "BRIGHTNESS" => {
            let percent = value
                .parse()
                .ok()
                .filter(|p| BRIGHTNESS.contains(p) && p % BRIGHTNESS_STEP == 0)?;
            return Some(Change::Brightness(percent));
        }
        "ZONE" => {
            options.zone = match value {
                "CHILE" => Zone::Chile,
                offset => {
                    let hours: i8 = offset.strip_prefix("UTC")?.parse().ok()?;
                    Zone::decode(Zone::Fixed(hours.checked_mul(4)?).encode())?
                }
            }
        }
        _ => return None,
    }
    if !alerts.valid() {
        return None;
    }
    options.alerts = alerts.encode();
    Some(Change::Options(options))
}
