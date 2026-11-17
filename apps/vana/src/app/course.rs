//! An upload empties the course while it streams in, so one that fails or is
//! cut off reads the stored course back.
use super::Runtime;
use crate::kept::course::Seen;
use crate::ride::course::Course;
use device_api::bulk::ReadWrite;

impl Runtime {
    /// Read the stored course when it is unread, unless an upload is
    /// replacing it.
    pub(super) fn service_course(&mut self, media: &mut impl ReadWrite, now: u64) {
        let seen = Seen {
            ready: self.ride.bulk.status().usable(),
            uploading: self.phone_uploading(),
        };
        let Some(track) = self.track.as_deref_mut() else {
            return;
        };
        if !track
            .kept
            .service(seen, media, &mut *track.course, now)
            .read
        {
            return;
        }
        track.course_changed(media);
        self.display.redraw();
    }

    /// The loaded course, while one is.
    pub fn course(&self) -> Option<&Course> {
        self.track
            .as_deref()
            .map(|track| &*track.course)
            .filter(|course| !course.is_empty())
    }
}
