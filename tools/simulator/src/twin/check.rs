//! Whether a replay recorded the world its trace holds: the faithful check.
//!
//! A replay binds the same recorders around its servers that the device
//! binds around its drivers, so what it hands VANA is recorded again. Pass
//! by pass, the world records and the panel digests it wrote must be the
//! trace's own, byte for byte. Health, logs and what else a device measures
//! of itself are no input and are left out, and so are the frames of the
//! phone page when the trace replaced the credentials it draws.
use super::script::{self, Hex, Named};
use device_api::capability::Kind;
use firmware_services::trace::media::Access;
use firmware_services::trace::{self, End, Event, Events};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// Media records that matched, the settings region aside.
    pub physical: u64,
    /// Settings records that matched.
    pub semantic: u64,
    pub redacted_frames: u32,
    pub refused_frames: u32,
    /// The pass of the first record that differs, and how.
    pub difference: Option<(u64, String)>,
    /// That record is a frame: the rider saw other pixels.
    pub frame: bool,
}

struct Compared<'a> {
    events: Events<'a>,
    redacted: bool,
    pass_ms: u64,
    phone: bool,
    redacted_frames: u32,
    refused_frames: u32,
}

impl<'a> Iterator for Compared<'a> {
    type Item = (u64, Event<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let event = self.events.next()?;
            match event {
                Event::Pass { now_ms } => self.pass_ms = now_ms,
                Event::Page(page) => {
                    self.phone = page == "phone";
                    continue;
                }
                Event::Panel(_) if self.redacted && self.phone => {
                    self.redacted_frames += 1;
                    continue;
                }
                Event::Call(call) if Some(call.method) == script::named("Display", "submit") => {
                    self.refused_frames += 1;
                }
                Event::Panel(_)
                | Event::Call(_)
                | Event::Command(_)
                | Event::Forget(_)
                | Event::Session { .. }
                | Event::Repeated { .. }
                | Event::TooLarge { .. }
                | Event::PoweredOff
                | Event::NetworkTime(_) => {}
                _ => continue,
            }
            return Some((self.pass_ms, event));
        }
    }
}

fn shown(event: &Event<'_>) -> String {
    match event {
        Event::Call(call) => {
            format!("{} {}", Named(call.method), Hex(call.body))
        }
        other => format!("{other:?}"),
    }
}

/// Compare what a replay `replayed` with the trace it ran. A replay that
/// `parted` from its trace ended early: what it recorded until then is
/// still compared.
pub fn rerecorded(recorded: &[u8], replayed: &[u8], redacted: bool, parted: bool) -> Checked {
    let mut checked = Checked::default();
    let compared = |bytes| {
        trace::decode(bytes).map(|(_, events)| Compared {
            events,
            redacted,
            pass_ms: 0,
            phone: false,
            redacted_frames: 0,
            refused_frames: 0,
        })
    };
    let (mut recorded, mut replayed) = match (compared(recorded), compared(replayed)) {
        (Ok(recorded), Ok(replayed)) => (recorded, replayed),
        (Err(error), _) => {
            checked.difference = Some((0, format!("the recorded trace does not decode: {error}")));
            return checked;
        }
        (_, Err(error)) => {
            checked.difference = Some((0, format!("the replayed trace does not decode: {error}")));
            return checked;
        }
    };
    loop {
        let (was, now) = (recorded.next(), replayed.next());
        let pass_ms = was.or(now).map_or(recorded.pass_ms, |(pass_ms, _)| pass_ms);
        checked.frame = matches!(was, Some((_, Event::Panel(_))));
        let difference = match (was, now) {
            (None, None) => break,
            (Some(_), None) if parted => break,
            (Some((_, was)), Some((_, now))) if was == now => {
                if let Event::Call(call) = was {
                    let media = matches!(
                        script::declared(call.method).map(|(_, method)| method.kind),
                        Some(Kind::Media(_))
                    );
                    let inside = Access::of(&call).is_some_and(|access| !access.outside);
                    checked.physical += u64::from(media && inside);
                    checked.semantic += u64::from(call.method.capability == super::world::SETTINGS);
                }
                continue;
            }
            (Some((_, was)), Some((_, now))) => {
                format!("recorded {}, replayed {}", shown(&was), shown(&now))
            }
            (Some((_, was)), None) => format!("recorded {}, and the replay did not", shown(&was)),
            (None, Some((_, now))) => format!("the replay went on with {}", shown(&now)),
        };
        checked.difference = Some((pass_ms, format!("pass at {pass_ms} ms: {difference}")));
        break;
    }
    checked.redacted_frames = recorded.redacted_frames;
    checked.refused_frames = recorded.refused_frames;
    if checked.difference.is_none() {
        for (events, which) in [
            (&recorded.events, "recorded"),
            (&replayed.events, "replayed"),
        ] {
            if events.end() == Some(End::Corrupt) {
                let pass_ms = recorded.pass_ms;
                checked.difference = Some((
                    pass_ms,
                    format!("the {which} trace is damaged after the pass at {pass_ms} ms"),
                ));
                break;
            }
        }
    }
    checked
}
