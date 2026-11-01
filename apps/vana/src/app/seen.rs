use crate::ride::log::Status;
use crate::screens::workout::Page;
use core::fmt::Write;

/// One change the rider can see. Text borrows VANA until the sink returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen<'a> {
    /// The page by name; a ride page as `ride/<page>`, the paused screen as
    /// `ride/paused`.
    Page(&'a str),
    /// The toast band's text; empty once it clears.
    Toast(&'a str),
    /// The recorder's state by name; a pause the device chose is
    /// `auto_paused`.
    RideState(&'a str),
    /// The main page's readings as `name=value` pairs, e.g.
    /// `time=0:40:12 speed=31.2 power=-- hr=142 cad=88 dist=12.40 grade=4.0%`.
    Shown(&'a str),
    /// A note of a cue the sound accepted: the cue by name (`radar_clear`),
    /// its pattern, and the note's place in the cue (`note` of `of`, from 1).
    Note {
        cue: &'a str,
        pattern: u8,
        note: u8,
        of: u8,
    },
}

#[derive(Default)]
pub(super) struct Reported {
    page: &'static str,
    toast: Line<24>,
    ride: &'static str,
    second: Option<u64>,
    shown: Line<96>,
}

impl super::Runtime {
    /// Hand `sink` what the rider sees that changed since the last call, and
    /// the cue note the rider heard. Call once a pass, after `alert`. Reads
    /// VANA; writes only what it last reported.
    pub fn report_seen(&mut self, now: u64, sink: &mut impl FnMut(Seen<'_>)) {
        let page = self.seen_page();
        if self.reported.page != page {
            self.reported.page = page;
            sink(Seen::Page(page));
        }
        let mut toast = Line::default();
        if let Some(shown) = self.toast_at(now) {
            toast.push(shown.text());
        }
        if self.reported.toast != toast {
            sink(Seen::Toast(toast.as_str()));
            self.reported.toast = toast;
        }
        let ride = match self.ride.active().status() {
            Status::Paused if self.ride.auto.paused() => "auto_paused",
            status => status.name(),
        };
        if self.reported.ride != ride {
            self.reported.ride = ride;
            sink(Seen::RideState(ride));
        }
        let second = now / 1000;
        if self.navigation.page == Page::Ride && self.reported.second != Some(second) {
            self.reported.second = Some(second);
            let mut shown = Line::default();
            let _ = crate::screens::ride::shown(
                &mut shown,
                self.ride.active_ms(now) / 1000,
                &super::presentation::shown_sample(&self.readings),
                self.ride.totals.distance_mm(),
            );
            if self.reported.shown != shown {
                sink(Seen::Shown(shown.as_str()));
                self.reported.shown = shown;
            }
        }
        if let Some(played) = self.cues.take_played() {
            sink(Seen::Note {
                cue: played.cue.name(),
                pattern: played.pattern,
                note: played.note,
                of: played.of,
            });
        }
    }

    fn seen_page(&self) -> &'static str {
        let navigation = &self.navigation;
        match navigation.page {
            Page::Ride
                if self.ride.active().status() == Status::Paused && !navigation.pause_hidden =>
            {
                "ride/paused"
            }
            Page::Ride => navigation.ride_page.name(),
            page => page.name(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Line<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Default for Line<N> {
    fn default() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
}

impl<const N: usize> Line<N> {
    fn push(&mut self, bytes: &[u8]) {
        let take = bytes.len().min(N - self.len);
        self.bytes[self.len..self.len + take].copy_from_slice(&bytes[..take]);
        self.len += take;
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for Line<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push(s.as_bytes());
        Ok(())
    }
}
