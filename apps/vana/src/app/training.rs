//! Structured training in the runtime: the picker, the workout a ride
//! plays, its beeps, toasts and laps, the FTP sheet and workout uploads.
//!
//! A ride starts its player when the ride starts, from the workout chosen
//! on the picker; the player then advances inside `Ride::count` with the
//! ride totals. Each step that ends closes a lap and announces the next
//! step; the last three seconds of a timed step beep. While a workout runs,
//! MARK (TopLeft) ends the step, which also closes the lap.
//!
//! Uploads are codec files a host puts under `workouts/` on the FILES
//! volume (`cycling workout-load`), or the phone page does. The library
//! lists them at boot, each time the picker opens and on `WORKOUT LIST`,
//! keeping only names and outlines; a ride reads its workout's steps when it starts. A finished
//! ride's workout outcome is saved beside it (`workout::history`).
//!
//! Terminal:
//!
//! - `WORKOUT LIST` lists the uploads again and reports the library;
//!   `WORKOUT STATUS` reports the player.
//! - `WORKOUT SELECT row` chooses a picker row (0 is free ride) for the
//!   next ride and opens Ready, as the picker does; `WORKOUT DELETE n`
//!   deletes upload `n`'s file.
//! - `WORKOUT HISTORY INTERNAL|BULK id` reports a saved ride's workout.
use super::Runtime;
use super::cues::Cue;
use super::navigation::To;
use crate::ride::zones::TrainingProfile;
use crate::screens::chrome::Tone;
use crate::screens::kinetic::Kinetic;
use crate::screens::ride::{Deck, RidePage, Training as Shown, summary};
use crate::screens::touch::List;
use crate::screens::workout::{Page, Text};
use crate::screens::workouts::{Choice, Picker, VISIBLE};
use crate::workout::ftp::{Candidates, Question, Source};
use crate::workout::history;
use crate::workout::library::{Id, Library, LoadError, Outline};
use crate::workout::model::{Length, Name, Played};
use crate::workout::player::{Event, Events, Player, Summary};
use core::fmt::Write;
use device_api::bulk::ReadWrite;
use device_api::input::Role;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Beep {
    /// The last seconds of a step.
    Count,
    /// A step began, the workout ended or a test measured FTP.
    Step,
}

#[derive(Default)]
pub(super) struct Training {
    pub(super) library: Library,
    /// The workout the next ride plays; `None` rides free.
    pub(super) selected: Option<Id>,
    pub(super) picker_cursor: usize,
    /// The picker's scroll, which follows the finger.
    pub(super) picker_scroll: Kinetic,
    beep: Option<(Beep, u64)>,
    pub(super) ftp: Candidates,
    /// The library lists what FILES holds now; cleared at boot and each
    /// time the picker opens.
    listed: bool,
    /// Why the last listing found no uploads, if it failed.
    pub(super) list_error: Option<LoadError>,
    /// The picker was open at the last tick.
    picking: bool,
    /// The ride just started free: its workout could not be read.
    pub(super) missing: bool,
    /// The last workout ridden and how it went, once its ride is closed.
    pub(super) last: Option<(Name, Summary)>,
    /// The workout this ride plays, kept after it ends until the rider
    /// leaves the ride screen or starts the next ride.
    pub(super) player: Option<Player>,
    /// What the player reported during the last steps, for the runtime.
    pub(super) events: Events,
}

state!(Training {
    library,
    selected,
    picker_cursor,
    picker_scroll,
    beep,
    ftp,
    listed,
    list_error,
    picking,
    missing,
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

    /// The picker rows: free ride, then the library, as `Id`s.
    fn row(&self, row: usize) -> Option<Option<Id>> {
        if row == 0 {
            return Some(None);
        }
        self.library.ids().nth(row - 1).map(Some)
    }

    pub(super) fn rows(&self) -> usize {
        1 + self.library.ids().count()
    }

    /// List the uploads again, keeping the chosen one chosen if it is
    /// still there.
    pub(super) fn list(&mut self, media: &mut impl ReadWrite) {
        let chosen = match self.selected {
            Some(Id::Uploaded(index)) => self
                .library
                .uploads()
                .get(usize::from(index))
                .map(|upload| upload.file),
            _ => None,
        };
        self.list_error = self.library.refresh(media).err();
        if let Some(file) = chosen {
            self.selected = self.library.find(&file);
        }
        self.picker_cursor = self.picker_cursor.min(self.rows() - 1);
        self.listed = true;
    }

    /// The chosen workout's player, its steps read now: `Ok(None)` rides
    /// free.
    pub(super) fn load_chosen(
        &self,
        media: &mut impl ReadWrite,
    ) -> Result<Option<Player>, LoadError> {
        let Some(id) = self.selected else {
            return Ok(None);
        };
        self.library.load(id, media).map(|w| Some(Player::new(w)))
    }
}

/// "52 MIN  12 STEPS", or what an FTP test is.
fn detail(outline: Outline) -> Text<24> {
    let mut text = Text::new();
    let minutes = outline.planned_secs.div_ceil(60);
    let _ = match outline.test {
        Some(test) => write!(text, "{}", test.label()),
        None if outline.untimed => write!(text, "{} STEPS", outline.played),
        None => write!(text, "{minutes} MIN  {} STEPS", outline.played),
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
        self.training.player.is_some() || self.training.selected.is_some()
    }

    /// List the uploads when the library is out of date: at boot and when
    /// the picker opens, never during a ride.
    pub(super) fn service_workouts(&mut self, media: &mut impl ReadWrite) {
        let picking = self.navigation.page() == Page::Workouts;
        if picking && !self.training.picking {
            self.training.listed = false;
        }
        self.training.picking = picking;
        if self.training.listed || self.recorder_busy() {
            return;
        }
        self.training.list(media);
        self.display.redraw();
    }

    /// Delete upload `index`'s file. Later uploads move up a place: the
    /// chosen one stays chosen, unless it is the one deleted.
    pub(super) fn delete_upload(
        &mut self,
        index: usize,
        media: &mut impl ReadWrite,
    ) -> Result<(), LoadError> {
        let id = Id::Uploaded(index as u8);
        self.training.library.delete(id, media)?;
        self.training.selected = match self.training.selected {
            Some(chosen) if chosen == id => None,
            Some(Id::Uploaded(later)) if usize::from(later) > index => {
                Some(Id::Uploaded(later - 1))
            }
            other => other,
        };
        self.training.picker_cursor = self.training.picker_cursor.min(self.training.rows() - 1);
        self.display.redraw();
        Ok(())
    }

    /// Leaving the ride screen: the workout's summary stays readable, the
    /// next ride is free unless a workout is chosen again.
    pub(super) fn leave_workout(&mut self) {
        if let Some(player) = self.training.player.take() {
            self.training.last = Some((player.workout().name, player.summary()));
        }
        self.training.selected = None;
    }

    /// The picker scrolled to `offset`, with the card under a resting finger.
    pub(super) fn picker(&self, offset: i32, pressed: Option<usize>) -> Picker {
        let training = &self.training;
        let chosen = training
            .selected
            .and_then(|id| training.library.ids().position(|other| other == id))
            .map_or(0, |index| index + 1);
        let mut picker = Picker::new(
            &self.layout.workouts.list,
            training.rows(),
            training.picker_cursor,
            chosen,
            offset,
        );
        picker.pressed = pressed;
        let mut free = Text::new();
        let _ = free.push_str("NO TARGETS");
        picker.push(Choice {
            name: Name::new("FREE RIDE"),
            detail: free,
        });
        for id in training.library.ids() {
            if let Some((name, outline)) = training.library.outline(id) {
                picker.push(Choice {
                    name,
                    detail: detail(outline),
                });
            }
        }
        picker
    }

    /// MOVE focuses the next card (held, the next page of cards; past the
    /// last it wraps to the first), ACT chooses it for Ready, BACK goes Home.
    /// Either press first brings a focus that scrolled away back into view.
    pub(super) fn workouts_input(&mut self, button: Role, now: u64, held: bool) {
        match button {
            Role::Move => self.list_move(List::Workouts, VISIBLE, held, now),
            Role::Act if self.list_act(List::Workouts, now) => {
                self.choose_workout(self.training.picker_cursor)
            }
            Role::Back => self.navigation.go(To::Page(Page::Home)),
            _ => {}
        }
    }

    /// Choose picker row `row`: Ready names it, and the next ride plays it.
    pub(super) fn choose_workout(&mut self, row: usize) {
        let Some(id) = self.training.row(row) else {
            return;
        };
        self.training.picker_cursor = row;
        self.training.selected = id;
        self.navigation.go(To::Page(Page::Ready));
    }

    /// Yes saves a test's FTP, no keeps the one in force.
    pub(super) fn answer_ftp(&mut self, yes: bool, now: u64) {
        let Some(Question::Test { ftp, current }) = self.ftp_question() else {
            return;
        };
        match yes {
            true => {
                let mut profile = self.preferences.profile;
                profile.ftp_watts = ftp;
                self.preferences.set(profile, now);
                let mut text = Text::<20>::new();
                let _ = write!(text, "FTP {ftp} W SAVED");
                self.navigation.toast(text.as_str(), Tone::Good, now);
            }
            false => {
                let mut text = Text::<20>::new();
                let _ = write!(text, "FTP KEPT {current} W");
                self.navigation.toast(text.as_str(), Tone::Info, now);
            }
        }
        self.training.ftp.answered(Source::WorkoutTest);
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
                Event::Ftp(ftp) => {
                    self.training.beep(Beep::Step, now);
                    if ftp != self.preferences.profile.ftp_watts {
                        self.training.ftp.set(Source::WorkoutTest, Some(ftp));
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
        self.ride_pages().first()
    }

    pub(super) fn ride_pages(&self) -> Deck {
        self.deck_with_course_map().shown(self.workout_pages())
    }

    pub(super) fn deck_with_course_map(&self) -> Deck {
        let deck = self.preferences.options.deck;
        match self.course() {
            Some(_) => deck.showing(RidePage::Map),
            None => deck,
        }
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
        let number = |index: usize| arguments.get(index).and_then(|v| v.parse::<usize>().ok());
        let written = match (operation, arguments.len()) {
            (None | Some("LIST"), 0) => {
                if !self.recorder_busy() {
                    self.training.list(media);
                    self.display.redraw();
                }
                self.workout_list(output)
            }
            (Some("STATUS"), 0) => self.workout_status(output),
            (Some("SELECT"), 1) => {
                let Some(row) = number(0).filter(|&row| row < self.training.rows()) else {
                    return "INVALID";
                };
                if self.recorder_busy() {
                    return "STATE";
                }
                // As choosing it on the picker: Ready names it.
                self.choose_workout(row);
                self.display.redraw();
                write!(output, "selected={row}")
            }
            (Some("DELETE"), 1) => {
                let Some(index) = number(0) else {
                    return "INVALID";
                };
                if self.recorder_busy() {
                    return "STATE";
                }
                let Some(upload) = self.training.library.uploads().get(index).copied() else {
                    return "BOUNDS";
                };
                if let Err(error) = self.delete_upload(index, media) {
                    let _ = write!(output, "error={}", error.name());
                    return "FAILED";
                }
                write!(output, "deleted={}", upload.file.as_str())
            }
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

    fn workout_list(&self, output: &mut impl Write) -> core::fmt::Result {
        let library = &self.training.library;
        let selected = self
            .training
            .selected
            .and_then(|id| library.ids().position(|other| other == id))
            .map_or(0, |index| index + 1);
        let files = match self.training.list_error {
            None => "ok",
            Some(error) => error.name(),
        };
        write!(
            output,
            "rows={} selected={selected} files={files} uploads={} skipped={}",
            self.training.rows(),
            library.uploads().len(),
            library.skipped
        )?;
        for (index, upload) in library.uploads().iter().enumerate() {
            write!(
                output,
                " upload{index}={}:{}",
                upload.file.as_str(),
                Underscored(upload.name)
            )?;
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
            " laps={} skipped={} in_pct={} ftp={} offer={}",
            self.ride.laps().count(),
            summary.skipped,
            summary.in_target_pct().map_or(-1, i16::from),
            summary.ftp.unwrap_or(0),
            self.training.ftp.test().unwrap_or(0)
        )
    }

    pub(super) fn ftp_question(&self) -> Option<Question> {
        let offer_page = self.navigation.page() == Page::Summary
            && self.navigation.summary_page() == summary::pages(self.has_elevation())
            && self.ride_saved();
        self.training
            .ftp
            .question(self.preferences.profile.ftp_watts, offer_page)
    }

    /// The FTP sheet's numbers while it is up.
    pub(super) fn ftp_sheet(&self) -> Option<(u16, u16)> {
        match self.ftp_question()? {
            Question::Test { ftp, current } => Some((ftp, current)),
            Question::Summary(_) => None,
        }
    }

    /// The Home ride card names the workout the next ride plays.
    pub(super) fn chosen_name(&self) -> Option<Name> {
        let id = self.training.selected?;
        Some(self.training.library.outline(id)?.0)
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
