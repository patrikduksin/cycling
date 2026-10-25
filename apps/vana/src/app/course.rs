//! The course in the app: read from the FILES volume at boot and again when
//! the card becomes ready, replaced by an upload from the phone, cleared from
//! the phone. The course itself lives beside the breadcrumb (`back`); this
//! keeps whether it matches the stored file. An upload empties the course
//! while it streams in, so one that fails or is cut off reads the stored
//! course back. While the course is followed, a turn ahead is a toast and a
//! cue.
use super::Runtime;
use super::back::NUDGE_MS;
use super::cues::Cue;
use crate::ride::course::turns::Warning;
use crate::ride::course::{self, Course};
use crate::screens::chrome::{Toast, Tone};
use crate::screens::ride::turn_toast;
use device_api::bulk::ReadWrite;

/// A failed read is tried again this long after.
const RETRY_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Store {
    /// Not read yet, or the course in memory is not the stored one.
    #[default]
    Unread,
    /// The course in memory is the stored one, or there is none.
    Kept,
    /// No card, no FILES volume or no formatted file store.
    Unavailable,
    /// A file is there that does not decode; it stays until replaced.
    Damaged,
}

impl Store {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Unread => "reading",
            Self::Kept => "kept",
            Self::Unavailable => "unavailable",
            Self::Damaged => "damaged",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Kept {
    pub(super) store: Store,
    retry_at: u64,
    /// The bulk medium was usable at the last service.
    card_ready: bool,
}

impl Kept {
    /// The result of reading or writing the stored course.
    pub(super) fn took(&mut self, result: Result<(), course::Error>, now: u64) {
        self.store = match result {
            Ok(()) => Store::Kept,
            Err(course::Error::Media) => {
                self.retry_at = now.saturating_add(RETRY_MS);
                Store::Unread
            }
            Err(course::Error::Damaged) => Store::Damaged,
            Err(course::Error::Unavailable | course::Error::Full) => Store::Unavailable,
        };
    }
}

impl Runtime {
    /// Read the stored course when it is unread, unless an upload is
    /// replacing it.
    pub(super) fn service_course(&mut self, media: &mut impl ReadWrite, now: u64) {
        let ready = self.ride.bulk.status().usable();
        let uploading = self.phone_uploading();
        let Some(back) = self.back.as_deref_mut() else {
            return;
        };
        let kept = &mut back.kept;
        if ready && !kept.card_ready && kept.store == Store::Unavailable {
            kept.store = Store::Unread;
        }
        kept.card_ready = ready;
        if kept.store != Store::Unread || uploading || now < kept.retry_at {
            return;
        }
        let result = course::load(media, back.course);
        if let Err(error) = result {
            log::warn!(target: "sdk.course", "load_failed error={error:?}");
        }
        back.kept.took(result, now);
        back.course_changed(media);
        self.display.redraw();
    }

    /// Warn of a turn on the course: LEFT IN 150 M, then LEFT at it.
    pub(super) fn warn_turn(&mut self, warning: Warning, now: u64) {
        let toast = Toast::new(turn_toast(warning).as_str(), Tone::Info);
        self.navigation.show(toast, NUDGE_MS, now);
        let cue = match warning {
            Warning::Ahead { .. } => Cue::TurnAhead,
            Warning::Now { .. } => Cue::Turn,
        };
        self.cue(cue, now);
        self.display.redraw();
    }

    /// The loaded course, while one is.
    pub fn course(&self) -> Option<&Course> {
        self.back
            .as_deref()
            .map(|back| &*back.course)
            .filter(|course| !course.is_empty())
    }
}
