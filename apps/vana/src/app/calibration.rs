//! Zeroing the saved power meter.
//!
//! One request is in flight at a time, moving through `Phase`: idle, requested
//! (the radio has not taken the message yet), waiting (the radio took it and
//! the power meter has not answered), then done with the offset it reported or
//! failed. `step` is the whole transition table; `Runtime` only turns radio
//! observations into events and `Requested` into a send. Nothing is stored.
use super::Runtime;
use crate::screens::sensors::CalibrationNote;
use crate::sensors::ant::device_type;
use crate::sensors::calibration::{self, Response};
use device_api::ant::{
    Admission, Ant, AntOperation, Error, Identity, LinkState, OperationId, Packet, SendStage,
    Target,
};

/// From the radio taking the request to the power meter's answer.
pub(super) const TIMEOUT_MS: u64 = 10_000;
/// How long the radio may take to accept the request. A finishing search
/// holds it for about this long, and cancelling that search holds it longer.
pub(super) const ADMISSION_MS: u64 = 20_000;
/// How long a result stays on the sensor page before it clears.
pub(super) const RESULT_MS: u64 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Failure {
    /// A ride is recording, and zeroing under load would corrupt it.
    WhileRiding,
    /// There is no live saved power meter to ask.
    NotConnected,
    /// The radio refused or lost the request.
    NotSent,
    /// The power meter answered that it could not zero.
    Rejected,
    /// The power meter never answered.
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    Idle,
    Requested {
        target: Target,
        since: u64,
    },
    Waiting {
        target: Target,
        id: OperationId,
        since: u64,
    },
    Done {
        offset: i16,
        at: u64,
    },
    Failed {
        failure: Failure,
        at: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Event {
    /// The rider asked; `live` is the saved power meter if it is connected.
    Ask {
        live: Option<Target>,
        recording: bool,
        now: u64,
    },
    /// The radio took the message.
    Sent { id: OperationId, now: u64 },
    /// The radio refused the message.
    Refused { error: Error, now: u64 },
    /// The radio's account of a message it took.
    Stage {
        id: OperationId,
        stage: SendStage,
        now: u64,
    },
    /// A calibration page arrived from this peer on this connection.
    Heard {
        identity: Identity,
        generation: u32,
        response: Response,
        now: u64,
    },
    /// Time passed, with the saved power meter's connection as it now is.
    Tick { live: Option<Target>, now: u64 },
}

pub(super) fn step(phase: Phase, event: Event) -> Phase {
    match (phase, event) {
        (Phase::Requested { .. } | Phase::Waiting { .. }, Event::Ask { .. }) => phase,
        (
            _,
            Event::Ask {
                recording: true,
                now,
                ..
            },
        ) => failed(Failure::WhileRiding, now),
        (
            _,
            Event::Ask {
                live: None, now, ..
            },
        ) => failed(Failure::NotConnected, now),
        (
            _,
            Event::Ask {
                live: Some(target),
                now,
                ..
            },
        ) => Phase::Requested { target, since: now },
        (Phase::Requested { target, .. }, Event::Sent { id, now }) => Phase::Waiting {
            target,
            id,
            since: now,
        },
        (
            Phase::Requested { .. },
            Event::Refused {
                error: Error::Busy | Error::InvalidState,
                ..
            },
        ) => phase,
        (Phase::Requested { .. }, Event::Refused { now, .. }) => failed(Failure::NotSent, now),
        (
            Phase::Waiting { id, .. },
            Event::Stage {
                id: seen,
                stage: SendStage::BridgeReplied { accepted: false } | SendStage::Cancelled,
                now,
            },
        ) if id == seen => failed(Failure::NotSent, now),
        (
            Phase::Waiting { target, .. },
            Event::Heard {
                identity,
                generation,
                response,
                now,
            },
        ) if identity == target.identity && generation == target.generation => match response {
            Response::Zeroed(offset) => Phase::Done { offset, at: now },
            Response::Failed => failed(Failure::Rejected, now),
        },
        (Phase::Requested { target, since }, Event::Tick { live, now }) => {
            if live != Some(target) {
                failed(Failure::NotConnected, now)
            } else if now.saturating_sub(since) >= ADMISSION_MS {
                failed(Failure::NotSent, now)
            } else {
                phase
            }
        }
        (Phase::Waiting { target, since, .. }, Event::Tick { live, now }) => {
            if live != Some(target) {
                failed(Failure::NotConnected, now)
            } else if now.saturating_sub(since) >= TIMEOUT_MS {
                failed(Failure::TimedOut, now)
            } else {
                phase
            }
        }
        (Phase::Done { at, .. } | Phase::Failed { at, .. }, Event::Tick { now, .. })
            if now.saturating_sub(at) >= RESULT_MS =>
        {
            Phase::Idle
        }
        _ => phase,
    }
}

const fn failed(failure: Failure, at: u64) -> Phase {
    Phase::Failed { failure, at }
}

impl Phase {
    /// What the sensor page tells the rider, if anything.
    pub(super) const fn note(self) -> Option<CalibrationNote> {
        match self {
            Self::Idle => None,
            Self::Requested { .. } | Self::Waiting { .. } => Some(CalibrationNote::Calibrating),
            Self::Done { offset, .. } => Some(CalibrationNote::Done(offset)),
            Self::Failed { failure, .. } => Some(match failure {
                Failure::WhileRiding => CalibrationNote::WhileRiding,
                Failure::NotConnected => CalibrationNote::NotConnected,
                Failure::NotSent | Failure::Rejected | Failure::TimedOut => CalibrationNote::Failed,
            }),
        }
    }
}

impl Runtime {
    /// The rider asked to zero the saved power meter.
    pub(super) fn calibrate(&mut self, ant: &mut impl Ant, now: u64) {
        let event = Event::Ask {
            live: self.live_power_meter(ant, now),
            recording: self.recording(),
            now,
        };
        self.calibration = step(self.calibration, event);
        self.submit_calibration(ant, now);
        self.display.redraw();
    }

    /// Take a power meter's calibration answer, if this packet is one.
    pub(super) fn hear_calibration(&mut self, packet: &Packet, now: u64) {
        if packet.identity.device_type != device_type::POWER {
            return;
        }
        let Some(response) = calibration::decode(packet.data) else {
            return;
        };
        let event = Event::Heard {
            identity: packet.identity,
            generation: packet.generation,
            response,
            now,
        };
        self.advance_calibration(event);
    }

    /// Offer a waiting request to the radio, follow its fate, and time it out.
    pub(super) fn service_calibration(&mut self, ant: &mut impl Ant, now: u64) {
        if let Phase::Waiting { id, .. } = self.calibration
            && let Some(observation) = ant.send_status(id)
        {
            self.advance_calibration(Event::Stage {
                id,
                stage: observation.stage,
                now,
            });
        }
        let live = self.live_power_meter(ant, now);
        self.advance_calibration(Event::Tick { live, now });
        self.submit_calibration(ant, now);
    }

    fn submit_calibration(&mut self, ant: &mut impl Ant, now: u64) {
        let Phase::Requested { target, .. } = self.calibration else {
            return;
        };
        let operation = AntOperation::Send {
            target,
            data: calibration::request(),
        };
        let event = match ant.request(operation, now) {
            Ok(Admission::SendQueued(id)) => Event::Sent { id, now },
            Ok(Admission::Accepted) => Event::Refused {
                error: Error::InvalidState,
                now,
            },
            Err(error) => Event::Refused { error, now },
        };
        self.advance_calibration(event);
    }

    fn advance_calibration(&mut self, event: Event) {
        let next = step(self.calibration, event);
        if next != self.calibration {
            self.calibration = next;
            self.display.redraw();
        }
    }

    /// The saved power meter, when its channel is connected and its data fresh.
    fn live_power_meter(&self, ant: &impl Ant, now: u64) -> Option<Target> {
        let (_, peer) = self.links.saved.of_type(device_type::POWER)?;
        let channel = ant.channel(device_type::POWER, now)?;
        (channel.selected == Some(peer) && channel.link == LinkState::Connected && !channel.stale)
            .then_some(Target {
                identity: peer,
                generation: channel.generation,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: Identity = Identity {
        device_type: device_type::POWER,
        device_number: 4242,
        transmission_type: 5,
    };
    const TARGET: Target = Target {
        identity: PEER,
        generation: 3,
    };
    const ID: OperationId = OperationId(7);
    const REQUESTED: Phase = Phase::Requested {
        target: TARGET,
        since: 1_000,
    };
    const WAITING: Phase = Phase::Waiting {
        target: TARGET,
        id: ID,
        since: 1_000,
    };

    fn ask(live: Option<Target>, recording: bool) -> Event {
        Event::Ask {
            live,
            recording,
            now: 1_000,
        }
    }

    fn heard(generation: u32, response: Response, now: u64) -> Event {
        Event::Heard {
            identity: PEER,
            generation,
            response,
            now,
        }
    }

    fn tick(live: Option<Target>, now: u64) -> Event {
        Event::Tick { live, now }
    }

    #[test]
    fn every_transition() {
        let other_peer = Identity {
            device_number: 1,
            ..PEER
        };
        let table: &[(&str, Phase, Event, Phase)] = &[
            (
                "ask from idle",
                Phase::Idle,
                ask(Some(TARGET), false),
                REQUESTED,
            ),
            (
                "ask while riding",
                Phase::Idle,
                ask(Some(TARGET), true),
                failed(Failure::WhileRiding, 1_000),
            ),
            (
                "ask with no live power meter",
                Phase::Idle,
                ask(None, false),
                failed(Failure::NotConnected, 1_000),
            ),
            (
                "riding wins over a missing power meter",
                Phase::Idle,
                ask(None, true),
                failed(Failure::WhileRiding, 1_000),
            ),
            (
                "ask again after a failure",
                failed(Failure::TimedOut, 500),
                ask(Some(TARGET), false),
                REQUESTED,
            ),
            (
                "ask again after a result",
                Phase::Done { offset: 9, at: 500 },
                ask(Some(TARGET), false),
                REQUESTED,
            ),
            (
                "ask while requested is ignored",
                REQUESTED,
                ask(Some(TARGET), false),
                REQUESTED,
            ),
            (
                "ask while waiting is ignored",
                WAITING,
                ask(Some(TARGET), true),
                WAITING,
            ),
            (
                "radio takes it, which starts the answer's clock",
                REQUESTED,
                Event::Sent { id: ID, now: 1_000 },
                WAITING,
            ),
            (
                "a slow radio moves the answer's deadline",
                REQUESTED,
                Event::Sent { id: ID, now: 6_000 },
                Phase::Waiting {
                    target: TARGET,
                    id: ID,
                    since: 6_000,
                },
            ),
            (
                "a sent event outside requested is ignored",
                WAITING,
                Event::Sent { id: ID, now: 9_000 },
                WAITING,
            ),
            (
                "radio busy retries",
                REQUESTED,
                Event::Refused {
                    error: Error::Busy,
                    now: 1_000,
                },
                REQUESTED,
            ),
            (
                "radio invalid state retries",
                REQUESTED,
                Event::Refused {
                    error: Error::InvalidState,
                    now: 1_000,
                },
                REQUESTED,
            ),
            (
                "radio refuses for good",
                REQUESTED,
                Event::Refused {
                    error: Error::Uncertain,
                    now: 1_000,
                },
                failed(Failure::NotSent, 1_000),
            ),
            (
                "stale generation refused",
                REQUESTED,
                Event::Refused {
                    error: Error::StaleGeneration,
                    now: 1_000,
                },
                failed(Failure::NotSent, 1_000),
            ),
            (
                "refusal outside requested is ignored",
                WAITING,
                Event::Refused {
                    error: Error::Unavailable,
                    now: 1_000,
                },
                WAITING,
            ),
            (
                "bridge rejects",
                WAITING,
                Event::Stage {
                    id: ID,
                    stage: SendStage::BridgeReplied { accepted: false },
                    now: 1_000,
                },
                failed(Failure::NotSent, 1_000),
            ),
            (
                "cancelled before the uart",
                WAITING,
                Event::Stage {
                    id: ID,
                    stage: SendStage::Cancelled,
                    now: 1_000,
                },
                failed(Failure::NotSent, 1_000),
            ),
            (
                "bridge accepts keeps waiting",
                WAITING,
                Event::Stage {
                    id: ID,
                    stage: SendStage::BridgeReplied { accepted: true },
                    now: 1_000,
                },
                WAITING,
            ),
            (
                "uncertain delivery keeps waiting",
                WAITING,
                Event::Stage {
                    id: ID,
                    stage: SendStage::Uncertain,
                    now: 1_000,
                },
                WAITING,
            ),
            (
                "another operation's stage is ignored",
                WAITING,
                Event::Stage {
                    id: OperationId(8),
                    stage: SendStage::Cancelled,
                    now: 1_000,
                },
                WAITING,
            ),
            (
                "success carries the offset",
                WAITING,
                heard(3, Response::Zeroed(-12), 2_000),
                Phase::Done {
                    offset: -12,
                    at: 2_000,
                },
            ),
            (
                "failure answer",
                WAITING,
                heard(3, Response::Failed, 2_000),
                failed(Failure::Rejected, 2_000),
            ),
            (
                "answer from an old connection is ignored",
                WAITING,
                heard(2, Response::Zeroed(5), 2_000),
                WAITING,
            ),
            (
                "answer from a newer connection is ignored",
                WAITING,
                heard(4, Response::Failed, 2_000),
                WAITING,
            ),
            (
                "answer from another peer is ignored",
                WAITING,
                Event::Heard {
                    identity: other_peer,
                    generation: 3,
                    response: Response::Zeroed(5),
                    now: 2_000,
                },
                WAITING,
            ),
            (
                "unprompted answer while idle is ignored",
                Phase::Idle,
                heard(3, Response::Zeroed(5), 2_000),
                Phase::Idle,
            ),
            (
                "answer before the radio took it is ignored",
                REQUESTED,
                heard(3, Response::Zeroed(5), 2_000),
                REQUESTED,
            ),
            (
                "waiting inside the bound",
                WAITING,
                tick(Some(TARGET), 1_000 + TIMEOUT_MS - 1),
                WAITING,
            ),
            (
                "silence times out",
                WAITING,
                tick(Some(TARGET), 1_000 + TIMEOUT_MS),
                failed(Failure::TimedOut, 1_000 + TIMEOUT_MS),
            ),
            (
                "a busy radio is waited for",
                REQUESTED,
                tick(Some(TARGET), 1_000 + ADMISSION_MS - 1),
                REQUESTED,
            ),
            (
                "a radio that never takes it gives up",
                REQUESTED,
                tick(Some(TARGET), 1_000 + ADMISSION_MS),
                failed(Failure::NotSent, 1_000 + ADMISSION_MS),
            ),
            (
                "connection lost while waiting",
                WAITING,
                tick(None, 2_000),
                failed(Failure::NotConnected, 2_000),
            ),
            (
                "new connection while waiting",
                WAITING,
                tick(
                    Some(Target {
                        generation: 4,
                        ..TARGET
                    }),
                    2_000,
                ),
                failed(Failure::NotConnected, 2_000),
            ),
            (
                "connection lost while requested",
                REQUESTED,
                tick(None, 2_000),
                failed(Failure::NotConnected, 2_000),
            ),
            (
                "result is held",
                Phase::Done { offset: 9, at: 500 },
                tick(None, 500 + RESULT_MS - 1),
                Phase::Done { offset: 9, at: 500 },
            ),
            (
                "result clears",
                Phase::Done { offset: 9, at: 500 },
                tick(None, 500 + RESULT_MS),
                Phase::Idle,
            ),
            (
                "failure clears",
                failed(Failure::Rejected, 500),
                tick(Some(TARGET), 500 + RESULT_MS),
                Phase::Idle,
            ),
            (
                "idle stays idle",
                Phase::Idle,
                tick(None, u64::MAX),
                Phase::Idle,
            ),
        ];
        for (name, from, event, want) in table {
            assert_eq!(step(*from, *event), *want, "{name}");
        }
    }

    #[test]
    fn the_state_stays_small() {
        assert!(
            core::mem::size_of::<Phase>() <= 32,
            "{}",
            core::mem::size_of::<Phase>()
        );
        assert!(core::mem::size_of::<Option<CalibrationNote>>() <= 4);
    }

    #[test]
    fn a_clock_that_runs_backwards_never_ends_a_wait() {
        assert_eq!(step(WAITING, tick(Some(TARGET), 0)), WAITING);
        assert_eq!(step(REQUESTED, tick(Some(TARGET), 0)), REQUESTED);
    }

    #[test]
    fn the_rider_is_told_only_what_is_true() {
        assert_eq!(Phase::Idle.note(), None);
        assert_eq!(REQUESTED.note(), Some(CalibrationNote::Calibrating));
        assert_eq!(WAITING.note(), Some(CalibrationNote::Calibrating));
        assert_eq!(
            Phase::Done { offset: -4, at: 0 }.note(),
            Some(CalibrationNote::Done(-4))
        );
        for failure in [Failure::NotSent, Failure::Rejected, Failure::TimedOut] {
            assert_eq!(failed(failure, 0).note(), Some(CalibrationNote::Failed));
        }
    }

    #[test]
    fn the_whole_lifecycle_in_order() {
        let mut phase = Phase::Idle;
        for event in [
            ask(Some(TARGET), false),
            Event::Refused {
                error: Error::Busy,
                now: 1_000,
            },
            Event::Sent { id: ID, now: 1_100 },
            heard(3, Response::Zeroed(512), 1_900),
            tick(Some(TARGET), 2_000),
        ] {
            phase = step(phase, event);
        }
        assert_eq!(
            phase,
            Phase::Done {
                offset: 512,
                at: 1_900
            }
        );
        assert_eq!(
            step(phase, tick(Some(TARGET), 1_900 + RESULT_MS)),
            Phase::Idle
        );
    }
}
