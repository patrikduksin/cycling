//! Structured training in the runtime: today's workout, the workout a ride
//! plays, its beeps, toasts and laps.
//!
//! Home's WORKOUT starts a ride that plays today's workout; the player then
//! advances inside `Ride::count` with the ride totals. Each step that ends
//! closes a lap and announces the next step; the last three seconds of a
//! timed step beep. While a workout runs, MARK (TopLeft) ends the step,
//! which also closes the lap.
//!
//! Today's workout is one codec file a host puts on the FILES volume
//! (`cycling workout-load`), or the phone page does (`workout::today`). It
//! is read at boot, on `WORKOUT LOAD` and after the phone changes it, and
//! read again every `READ_RETRY_MS` while the medium fails it. A
//! finished ride's workout outcome is saved beside it (`workout::history`).
//!
//! Terminal:
//!
//! - `WORKOUT LOAD` reads today's workout again and reports it;
//!   `WORKOUT STATUS` reports the player, or today's workout before a ride.
//! - `WORKOUT HISTORY INTERNAL|BULK id` reports a saved ride's workout.
use super::Runtime;
use super::cues::Cue;
use super::navigation::To;
use crate::ride::zones::TrainingProfile;
use crate::screens::chrome::Tone;
use crate::screens::ride::{Deck, RidePage, Training as Shown};
use crate::screens::workout::{Page, Text};
use crate::workout::history;
use crate::workout::model::{Length, Name, Played, Workout};
use crate::workout::player::{Event, Events, Player, Summary};
use crate::workout::today::{self, LoadError};
use core::fmt::Write;
use device_api::bulk::ReadWrite;
use device_api::input::Role;

/// How long after a read the medium failed today's workout is read again.
const READ_RETRY_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Beep {
    /// The last seconds of a step.
    Count,
    /// A step began, the workout ended or a test measured FTP.
    Step,
}

#[derive(Default)]
pub(super) struct Training {
    /// Today's workout as FILES held it when last read.
    pub(super) today: Option<Workout>,
    /// Why today's workout could not be read the last time.
    pub(super) read_error: Option<LoadError>,
    /// Today's workout, or that there is none, was read since boot. A read
    /// the medium failed is tried again from `retry_at`.
    read: bool,
    retry_at: u64,
    /// The next ride plays today's workout; otherwise it rides free.
    pub(super) chosen: bool,
    beep: Option<(Beep, u64)>,
    /// The last workout ridden and how it went, once its ride is closed.
    pub(super) last: Option<(Name, Summary)>,
    /// The workout this ride plays, kept after it ends until the rider
    /// leaves the ride screen or starts the next ride.
    pub(super) player: Option<Player>,
    /// What the player reported during the last steps, for the runtime.
    pub(super) events: Events,
}

state!(Training {
    today,
    read_error,
    read,
    retry_at,
    chosen,
    beep,
    last,
    player,
    events,
});

impl Training {
    fn beep(&mut self, beep: Beep, now: u64) {
        // A step beep outranks a countdown beep in the same moment.
        if self.beep.is_some_and(|(queued, _)| queued == Beep::Step) && beep == Beep::Count {
            return;
        }
        self.beep = Some((beep, now));
    }

    /// Read today's workout from FILES again. Only a workout or a sure
    /// "none" settles it; a read the medium failed leaves it to retry.
    pub(super) fn read_today(&mut self, media: &mut impl ReadWrite) {
        let read = today::read(media);
        self.today = read.ok().flatten();
        self.read_error = read.err();
        self.read = read.is_ok();
    }

    /// The player for a ride that starts now: today's workout if it was
    /// chosen, else none.
    pub(super) fn start(&self) -> Option<Player> {
        self.today.filter(|_| self.chosen).map(Player::new)
    }
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
        self.training
            .player
            .as_ref()
            .map(|player| (player.workout().name, player.summary()))
            .or(self.training.last)
    }

    /// The workout the ride screen shows: the one this ride plays.
    pub(super) fn shown_workout(&self, profile: &TrainingProfile) -> Option<Shown> {
        let player = self.training.player.as_ref()?;
        Some(Shown::of(player, profile))
    }

    /// The ride screen has a workout page.
    pub(super) fn workout_pages(&self) -> bool {
        self.training.player.is_some() || self.training.chosen
    }

    /// Read today's workout after boot, and again a while after a read the
    /// medium failed, as a card that recovers or is formatted reads again;
    /// never during a ride.
    pub(super) fn service_workouts(&mut self, media: &mut impl ReadWrite, now: u64) {
        if self.training.read || now < self.training.retry_at || self.recorder_busy() {
            return;
        }
        self.training.read_today(media);
        self.training.retry_at = now + READ_RETRY_MS;
        self.display.redraw();
    }

    /// Home's WORKOUT: start a ride that plays today's workout, as RIDE
    /// starts a free one. Without a workout it only says so.
    pub(super) fn ride_workout(&mut self, held: bool, pressed_at: u64, now: u64) {
        if self.training.today.is_none() {
            self.navigation.toast("NO WORKOUT LOADED", Tone::Info, now);
            return;
        }
        self.training.chosen = !self.recorder_busy();
        self.ride_press(Role::Act, held, pressed_at, now);
        self.training.chosen &= self.navigation.page() == Page::Ride;
    }

    /// Leaving the ride screen: the workout's summary stays readable, and
    /// today's workout, once ridden, leaves Home until a new one is loaded.
    /// The next ride is free unless Home's workout card starts it.
    pub(super) fn leave_workout(&mut self) {
        if let Some(player) = self.training.player.take() {
            self.training.last = Some((player.workout().name, player.summary()));
            self.training.today = None;
        }
        self.training.chosen = false;
    }

    /// MARK during a workout: the lap the press closed ends the step too.
    /// Returns whether the workout took the press.
    pub(super) fn workout_lap(&mut self, now: u64) -> bool {
        let Some(player) = self.training.player.as_mut().filter(|p| !p.done()) else {
            return false;
        };
        let events = player.skip();
        for event in events {
            let _ = self.training.events.push(event);
        }
        self.workout_events(now);
        true
    }

    /// A tap on the workout page's time left adds to the step.
    pub(super) fn step_extendable(&self) -> bool {
        self.training
            .player
            .as_ref()
            .is_some_and(|player| player.extendable())
    }

    pub(super) fn extend_step(&mut self, now: u64) {
        let Some(player) = self.training.player.as_mut() else {
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
        let events = core::mem::take(&mut self.training.events);
        for event in events {
            match event {
                Event::Countdown(_) => self.training.beep(Beep::Count, now),
                // The step's lap closed as it ended, or as MARK skipped it.
                Event::Started(_) | Event::Finished => {
                    self.training.beep(Beep::Step, now);
                    let profile = self.preferences.profile;
                    let played = self
                        .training
                        .player
                        .as_ref()
                        .filter(|player| !player.done())
                        .map(|player| player.now(&profile).played);
                    match played {
                        Some(played) => {
                            let text = step_toast(played);
                            self.navigation.toast(text.as_str(), Tone::Info, now);
                        }
                        None => {
                            // The workout page has nothing left to show.
                            self.navigation.go(To::WorkoutDone);
                            self.navigation.toast("WORKOUT DONE", Tone::Good, now);
                        }
                    }
                }
                Event::Ftp(_) => self.training.beep(Beep::Step, now),
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
        self.ride_pages().first()
    }

    pub(super) fn ride_pages(&self) -> Deck {
        Deck::new(self.workout_pages())
    }

    pub(super) fn workout_command(
        &mut self,
        words: &mut core::str::SplitAsciiWhitespace<'_>,
        media: &mut impl ReadWrite,
        output: &mut impl Write,
    ) -> &'static str {
        let operation = words.next();
        let arguments: heapless::Vec<&str, 2> = words.by_ref().take(2).collect();
        if words.next().is_some() {
            return "INVALID";
        }
        let written = match (operation, arguments.len()) {
            (Some("LOAD"), 0) => {
                if self.recorder_busy() {
                    return "STATE";
                }
                self.training.read_today(media);
                self.display.redraw();
                self.write_today(output)
            }
            (None | Some("STATUS"), 0) => self.workout_status(output),
            (Some("HISTORY"), 2) => {
                let journal = match arguments[0] {
                    "INTERNAL" => b'i',
                    "BULK" => b'b',
                    _ => return "INVALID",
                };
                let Some(ride) = arguments[1].parse::<u32>().ok() else {
                    return "INVALID";
                };
                match history::load(media, journal, ride) {
                    Ok(Some(record)) => write_record(output, &record),
                    Ok(None) => write!(output, "ride={ride} workout=none"),
                    Err(error) => {
                        let _ = write!(output, "error={}", error.name());
                        return "FAILED";
                    }
                }
            }
            _ => return "INVALID",
        };
        if written.is_err() { "OUTPUT" } else { "OK" }
    }

    /// Today's workout as reply fields: "workout=NAME minutes=52 steps=12
    /// files=ok", or "workout=none files=…".
    fn write_today(&self, output: &mut impl Write) -> core::fmt::Result {
        match &self.training.today {
            Some(workout) => write!(
                output,
                "workout={} minutes={} steps={}",
                Underscored(workout.name),
                workout.planned_secs().div_ceil(60),
                workout.played_len()
            )?,
            None => write!(output, "workout=none")?,
        }
        let files = self.training.read_error.map_or("ok", LoadError::name);
        write!(output, " files={files}")
    }

    fn workout_status(&self, output: &mut impl Write) -> core::fmt::Result {
        let profile = self.preferences.profile;
        let Some(shown) = self.shown_workout(&profile) else {
            return match &self.training.today {
                Some(workout) => write!(output, "workout={} running=0", Underscored(workout.name)),
                None => write!(output, "workout=none"),
            };
        };
        let now = shown.now;
        write!(
            output,
            "workout={} running={} step={}/{} index={} repetition={} done={}",
            Underscored(shown.name),
            u8::from(self.training.player.is_some()),
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
            .training
            .player
            .as_ref()
            .map(Player::summary)
            .unwrap_or_default();
        write!(
            output,
            " laps={} skipped={} in_pct={} ftp={}",
            self.ride.laps().count(),
            summary.skipped,
            summary.in_target_pct().map_or(-1, i16::from),
            summary.ftp.unwrap_or(0)
        )
    }
}

/// A saved ride's workout record as reply fields.
fn write_record(output: &mut impl Write, record: &history::Record) -> core::fmt::Result {
    let summary = &record.summary;
    write!(
        output,
        "ride={} workout={} test={} steps={}/{} skipped={} finished={} in_pct={} ftp={} start_utc_ms={}",
        record.ride_id,
        Underscored(record.name),
        record.test.map_or("none", |test| test.label()),
        summary.steps_done,
        summary.steps,
        summary.skipped,
        u8::from(summary.finished),
        summary.in_target_pct().map_or(-1, i16::from),
        summary.ftp.unwrap_or(0),
        record.started_utc_ms.unwrap_or(0),
    )
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
