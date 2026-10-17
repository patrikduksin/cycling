//! Structured training in the runtime: the picker, the workout a ride
//! plays, its beeps, toasts and laps, the FTP sheet and workout uploads.
//!
//! A ride starts its player when the ride starts, from the workout chosen
//! on the picker; the player then advances inside `Ride::count` with the
//! ride totals. Each step that ends closes a lap and announces the next
//! step; the last three seconds of a timed step beep. While a workout runs,
//! MARK (TopLeft) ends the step, which also closes the lap.
//!
//! Terminal, one workout at a time into upload slot `n` (`mise run
//! workout-load` drives it):
//!
//! - `WORKOUT LIST` and `WORKOUT STATUS` report the library and the player.
//! - `WORKOUT BEGIN n length crc32hex`, then `WORKOUT DATA offset hex` in
//!   order, then `WORKOUT COMMIT` checks, decodes and stores the workout.
//! - `WORKOUT DELETE n` empties a slot; `WORKOUT SELECT row` chooses a
//!   picker row (0 is free ride) for the next ride and opens Ready, as the
//!   picker does.
use super::Runtime;
use super::cues::Cue;
use crate::ride::zones::TrainingProfile;
use crate::screens::chrome::Tone;
use crate::screens::ride::{RidePage, Training as Shown};
use crate::screens::workout::{Page, Text};
use crate::screens::workouts::{Choice, Picker};
use crate::workout::codec::ENCODED_MAX;
use crate::workout::library::{Id, Library, UPLOAD_SLOTS};
use crate::workout::model::{Length, Name, Played};
use crate::workout::player::{Event, Player, Summary};
use core::fmt::Write;
use device_api::input::Button;
use firmware_services::checksum::crc32;

/// Bytes per `WORKOUT DATA` line: the console takes 128-byte domain commands.
pub const CHUNK: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Beep {
    /// The last seconds of a step.
    Count,
    /// A step began, the workout ended or a test measured FTP.
    Step,
}

struct Upload {
    slot: usize,
    length: usize,
    crc: u32,
    bytes: [u8; ENCODED_MAX],
    received: usize,
}

#[derive(Default)]
pub(super) struct Training {
    pub(super) library: Library,
    /// The workout the next ride plays; `None` rides free.
    pub(super) selected: Option<Id>,
    pub(super) picker_cursor: usize,
    beep: Option<(Beep, u64)>,
    /// A test's FTP waiting for the rider's answer.
    pub(super) ftp_offer: Option<u16>,
    upload: Option<Upload>,
    /// The last workout ridden and how it went, once its ride is closed.
    pub(super) last: Option<(Name, Summary)>,
}

impl Training {
    fn beep(&mut self, beep: Beep, now: u64) {
        // A step beep outranks a countdown beep in the same moment.
        if self.beep.is_some_and(|(queued, _)| queued == Beep::Step) && beep == Beep::Count {
            return;
        }
        self.beep = Some((beep, now));
    }

    /// The picker rows: free ride, then the library, as `Id`s.
    fn row(&self, row: usize) -> Option<Option<Id>> {
        if row == 0 {
            return Some(None);
        }
        self.library.ids().nth(row - 1).map(Some)
    }

    fn rows(&self) -> usize {
        1 + self.library.ids().count()
    }
}

/// "52 MIN  12 STEPS", or what an FTP test is.
fn detail(workout: &crate::workout::model::Workout) -> Text<24> {
    let mut text = Text::new();
    let minutes = workout.planned_secs().div_ceil(60);
    let _ = match workout.test {
        Some(test) => write!(text, "{}", test.label()),
        None if workout.has_untimed_steps() => write!(text, "{} STEPS", workout.played_len()),
        None => write!(text, "{minutes} MIN  {} STEPS", workout.played_len()),
    };
    text
}

/// The toast for a step that began: "WORK 2/5 3:00".
fn step_toast(played: Played) -> Text<20> {
    let mut text = Text::new();
    let _ = text.push_str(played.step.intensity.label());
    if played.of > 1 {
        let _ = write!(text, " {}/{}", played.repetition + 1, played.of);
    }
    match played.step.length {
        Length::Time(secs) => {
            let _ = write!(text, " {}:{:02}", secs / 60, secs % 60);
        }
        Length::Distance(m) => {
            let _ = write!(text, " {}.{}KM", m / 1000, m % 1000 / 100);
        }
        Length::Open => {}
    }
    text
}

impl Runtime {
    /// How the workout of the last or current ride went, and its name:
    /// steps done and skipped, time on, under and over target, and a test's
    /// FTP. `None` after a free ride.
    pub fn workout_summary(&self) -> Option<(Name, Summary)> {
        self.ride
            .workout
            .as_ref()
            .map(|player| (player.workout().name, player.summary()))
            .or(self.training.last)
    }

    /// The workout the ride screen shows: the one this ride plays.
    pub(super) fn shown_workout(&self, profile: &TrainingProfile) -> Option<Shown> {
        let player = self.ride.workout.as_ref()?;
        Some(Shown::of(player, profile))
    }

    /// The ride screen has a workout page.
    pub(super) fn workout_pages(&self) -> bool {
        self.ride.workout.is_some() || self.training.selected.is_some()
    }

    /// A new ride starts the chosen workout, if any.
    pub(super) fn chosen_player(&self) -> Option<Player> {
        let id = self.training.selected?;
        self.training.library.get(id).copied().map(Player::new)
    }

    /// Leaving the ride screen: the workout's summary stays readable, the
    /// next ride is free unless a workout is chosen again.
    pub(super) fn leave_workout(&mut self) {
        if let Some(player) = self.ride.workout.take() {
            self.training.last = Some((player.workout().name, player.summary()));
        }
        self.training.selected = None;
    }

    pub(super) fn picker(&self) -> Picker {
        let training = &self.training;
        let chosen = training
            .selected
            .and_then(|id| training.library.ids().position(|other| other == id))
            .map_or(0, |index| index + 1);
        let mut picker = Picker::new(training.rows(), training.picker_cursor, chosen);
        let mut free = Text::new();
        let _ = free.push_str("NO TARGETS");
        picker.push(Choice {
            name: Name::new("FREE RIDE"),
            detail: free,
        });
        for id in training.library.ids() {
            if let Some(workout) = training.library.get(id) {
                picker.push(Choice {
                    name: workout.name,
                    detail: detail(workout),
                });
            }
        }
        picker
    }

    /// MOVE focuses the next card, ACT chooses it for Ready, BACK goes Home.
    pub(super) fn workouts_input(&mut self, button: Button, now: u64) {
        let rows = self.training.rows();
        match button {
            Button::BottomLeft => {
                self.training.picker_cursor = (self.training.picker_cursor + 1) % rows
            }
            Button::BottomRight => self.choose_workout(self.training.picker_cursor, now),
            Button::TopLeft => self.navigation.page = Page::Home,
            _ => {}
        }
    }

    /// Choose picker row `row`: Ready names it, and the next ride plays it.
    pub(super) fn choose_workout(&mut self, row: usize, now: u64) {
        let Some(id) = self.training.row(row) else {
            return;
        };
        self.training.picker_cursor = row;
        self.training.selected = id;
        self.navigation.page = Page::Ready;
        self.navigation.page_since = now;
    }

    /// ACT saves a test's FTP, BACK keeps the one in force; MOVE does nothing.
    pub(super) fn ftp_input(&mut self, button: Button, now: u64) -> bool {
        let Some(ftp) = self.training.ftp_offer else {
            return false;
        };
        match button {
            Button::BottomRight => {
                let mut profile = self.preferences.profile;
                profile.ftp_watts = ftp;
                self.preferences.set(profile, now);
                let mut text = Text::<20>::new();
                let _ = write!(text, "FTP {ftp} W SAVED");
                self.navigation.toast(text.as_str(), Tone::Good, now);
            }
            Button::TopLeft => {
                let mut text = Text::<20>::new();
                let _ = write!(text, "FTP KEPT {} W", self.preferences.profile.ftp_watts);
                self.navigation.toast(text.as_str(), Tone::Info, now);
            }
            _ => return true,
        }
        self.training.ftp_offer = None;
        true
    }

    /// MARK during a workout: the lap the press closed ends the step too.
    /// Returns whether the workout took the press.
    pub(super) fn workout_lap(&mut self, now: u64) -> bool {
        let Some(player) = self.ride.workout.as_mut().filter(|p| !p.done()) else {
            return false;
        };
        let events = player.skip();
        for event in events {
            let _ = self.ride.workout_events.push(event);
        }
        self.workout_events(now);
        true
    }

    /// A tap on the workout page's time left adds to the step.
    pub(super) fn extend_step(&mut self, now: u64) {
        let Some(player) = self.ride.workout.as_mut() else {
            return;
        };
        let length = player.now(&self.preferences.profile).played.step.length;
        if player.extend() {
            let text = match length {
                Length::Distance(_) => "+0.5 KM",
                _ => "+1 MIN",
            };
            self.navigation.toast(text, Tone::Info, now);
        }
    }

    /// Act on what the player reported: beeps, laps, toasts, the FTP sheet.
    pub(super) fn workout_events(&mut self, now: u64) {
        let events = core::mem::take(&mut self.ride.workout_events);
        for event in events {
            match event {
                Event::Countdown(_) => self.training.beep(Beep::Count, now),
                // The step's lap closed as it ended, or as MARK skipped it.
                Event::Started(_) | Event::Finished => {
                    self.training.beep(Beep::Step, now);
                    let profile = self.preferences.profile;
                    let played = self
                        .ride
                        .workout
                        .as_ref()
                        .filter(|player| !player.done())
                        .map(|player| player.now(&profile).played);
                    match played {
                        Some(played) => {
                            let text = step_toast(played);
                            self.navigation.toast(text.as_str(), Tone::Info, now);
                        }
                        None => self.navigation.toast("WORKOUT DONE", Tone::Good, now),
                    }
                }
                Event::Ftp(ftp) => {
                    self.training.beep(Beep::Step, now);
                    if ftp != self.preferences.profile.ftp_watts {
                        self.training.ftp_offer = Some(ftp);
                    }
                }
            }
        }
        self.display.redraw();
    }

    /// Raise the cue the workout owes, as of when it was owed: cues drop
    /// one too late to mean anything.
    pub(super) fn workout_cue(&mut self) {
        if let Some((beep, at)) = self.training.beep.take() {
            let cue = match beep {
                Beep::Count => Cue::Countdown,
                Beep::Step => Cue::Step,
            };
            self.cue(cue, at);
        }
    }

    /// The ride page to open on the ride screen.
    pub(super) fn first_ride_page(&self) -> RidePage {
        if self.workout_pages() {
            RidePage::Workout
        } else {
            RidePage::Main
        }
    }

    pub(super) fn workout_command(
        &mut self,
        words: &mut core::str::SplitAsciiWhitespace<'_>,
        now: u64,
        output: &mut impl Write,
    ) -> &'static str {
        let operation = words.next();
        let arguments: heapless::Vec<&str, 3> = words.by_ref().take(3).collect();
        if words.next().is_some() {
            return "INVALID";
        }
        let number = |index: usize| arguments.get(index).and_then(|v| v.parse::<usize>().ok());
        let written = match (operation, arguments.len()) {
            (None | Some("LIST"), 0) => self.workout_list(output),
            (Some("STATUS"), 0) => self.workout_status(output),
            (Some("SELECT"), 1) => {
                let Some(row) = number(0).filter(|&row| row < self.training.rows()) else {
                    return "INVALID";
                };
                if self.recording() {
                    return "STATE";
                }
                // As choosing it on the picker: Ready names it.
                self.choose_workout(row, now);
                self.display.redraw();
                write!(output, "selected={row}")
            }
            (Some("BEGIN"), 3) => {
                let (Some(slot), Some(length), Ok(crc)) =
                    (number(0), number(1), u32::from_str_radix(arguments[2], 16))
                else {
                    return "INVALID";
                };
                if slot >= UPLOAD_SLOTS || length == 0 || length > ENCODED_MAX {
                    return "BOUNDS";
                }
                self.training.upload = Some(Upload {
                    slot,
                    length,
                    crc,
                    bytes: [0; ENCODED_MAX],
                    received: 0,
                });
                write!(output, "slot={slot} length={length} chunk={CHUNK}")
            }
            (Some("DATA"), 2) => {
                let Some(upload) = self.training.upload.as_mut() else {
                    return "STATE";
                };
                let hex = arguments[1].as_bytes();
                if number(0) != Some(upload.received) {
                    return "SEQUENCE";
                }
                if !hex.len().is_multiple_of(2) || hex.len() / 2 > CHUNK {
                    return "INVALID";
                }
                let end = upload.received + hex.len() / 2;
                if end > upload.length {
                    return "BOUNDS";
                }
                for (i, pair) in hex.chunks(2).enumerate() {
                    let digits = core::str::from_utf8(pair).ok();
                    let Some(byte) = digits.and_then(|d| u8::from_str_radix(d, 16).ok()) else {
                        return "INVALID";
                    };
                    upload.bytes[upload.received + i] = byte;
                }
                upload.received = end;
                write!(output, "received={end}")
            }
            (Some("COMMIT"), 0) => {
                let Some(upload) = self.training.upload.take() else {
                    return "STATE";
                };
                let bytes = &upload.bytes[..upload.length];
                if upload.received != upload.length || crc32(bytes) != upload.crc {
                    return "CHECKSUM";
                }
                match self.training.library.install(upload.slot, bytes) {
                    Ok(workout) => {
                        let name = workout.name;
                        self.display.redraw();
                        write!(output, "slot={} name={}", upload.slot, Underscored(name))
                    }
                    Err(error) => {
                        let _ = write!(output, "error={error:?}");
                        return "INVALID";
                    }
                }
            }
            (Some("DELETE"), 1) => {
                let Some(slot) = number(0) else {
                    return "INVALID";
                };
                if self.training.selected == Some(Id::Uploaded(slot as u8)) {
                    if self.recording() {
                        return "STATE";
                    }
                    self.training.selected = None;
                }
                if self.training.library.remove(slot).is_err() {
                    return "BOUNDS";
                }
                self.display.redraw();
                write!(output, "slot={slot}")
            }
            _ => return "INVALID",
        };
        if written.is_err() { "OUTPUT" } else { "OK" }
    }

    fn workout_list(&self, output: &mut impl Write) -> core::fmt::Result {
        let library = &self.training.library;
        let selected = self
            .training
            .selected
            .and_then(|id| library.ids().position(|other| other == id))
            .map_or(0, |index| index + 1);
        write!(output, "rows={} selected={selected}", self.training.rows())?;
        for slot in 0..UPLOAD_SLOTS {
            match library.get(Id::Uploaded(slot as u8)) {
                Some(workout) => write!(output, " slot{slot}={}", Underscored(workout.name))?,
                None => write!(output, " slot{slot}=-")?,
            }
        }
        Ok(())
    }

    fn workout_status(&self, output: &mut impl Write) -> core::fmt::Result {
        let profile = self.preferences.profile;
        let Some(shown) = self.shown_workout(&profile) else {
            // Chosen, not yet started.
            return match self.chosen_name() {
                Some(name) => write!(output, "workout={} running=0", Underscored(name)),
                None => write!(output, "workout=none"),
            };
        };
        let now = shown.now;
        write!(
            output,
            "workout={} running={} step={}/{} index={} repetition={} done={}",
            Underscored(shown.name),
            u8::from(self.ride.workout.is_some()),
            now.position + 1,
            now.steps,
            now.played.index,
            now.played.repetition,
            u8::from(shown.done),
        )?;
        match now.left {
            crate::workout::player::Left::Secs(s) => write!(output, " left_s={s}")?,
            crate::workout::player::Left::Metres(m) => write!(output, " left_m={m}")?,
            crate::workout::player::Left::Open(s) => write!(output, " open_s={s}")?,
        }
        if let Some(band) = now.band {
            write!(output, " target={}-{}", band.low, band.high)?;
        }
        if let Some(reading) = now.reading {
            write!(output, " reading={reading}")?;
        }
        if let Some(compliance) = now.compliance {
            write!(output, " compliance={compliance:?}")?;
        }
        let summary = self
            .ride
            .workout
            .as_ref()
            .map(Player::summary)
            .unwrap_or_default();
        write!(
            output,
            " laps={} skipped={} in_pct={} ftp={} offer={}",
            self.ride.laps.count(),
            summary.skipped,
            summary.in_target_pct().map_or(-1, i16::from),
            summary.ftp.unwrap_or(0),
            self.training.ftp_offer.unwrap_or(0)
        )
    }

    /// The FTP sheet's numbers while it is up.
    pub(super) fn ftp_sheet(&self) -> Option<(u16, u16)> {
        self.training
            .ftp_offer
            .map(|ftp| (ftp, self.preferences.profile.ftp_watts))
    }

    /// The Home ride card names the workout the next ride plays.
    pub(super) fn chosen_name(&self) -> Option<Name> {
        let id = self.training.selected?;
        Some(self.training.library.get(id)?.name)
    }
}

/// A name in a reply, spaces as underscores so replies stay word-separated.
struct Underscored(Name);

impl core::fmt::Display for Underscored {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in self.0.as_str().chars() {
            f.write_char(if c == ' ' { '_' } else { c })?;
        }
        Ok(())
    }
}
