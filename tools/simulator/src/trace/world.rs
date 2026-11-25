//! A world record as the trace tools read it: the
//! value one call gave VANA, typed by the method its capability table names.
use crate::twin::script::declared;
use device_api::ant;
use device_api::ble_transport;
use device_api::bulk;
use device_api::capability::{Kind, Media};
use device_api::input::{Edge, InputSnapshot};
use device_api::observation::{Availability, Error, Observation};
use device_api::positioning;
use device_api::power;
use device_api::sensors;
use device_api::server::{AccessPointState, Listen, PhoneLink, Reach};
use device_api::sound;
use firmware_services::trace::media::Access;
use firmware_services::trace::origin::Memory;
use firmware_services::trace::wire::{Reader, Wire};
use firmware_services::trace::{Call, Event, ServerRequest, StorageError, StorageOp};

/// An output the shell drives that can refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Panel,
    Backlight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum World<'a> {
    Edge(Edge),
    AntAvailability(Availability),
    AntScanning(bool),
    AntDiscovery {
        slot: u8,
        found: Option<ant::Discovery>,
    },
    AntLink {
        channel: u8,
        snapshot: Option<ant::Snapshot>,
    },
    AntPacket(ant::Packet),
    AntRequest {
        operation: ant::AntOperation,
        result: Result<ant::Admission, ant::Error>,
    },
    AntSend(Option<ant::SendObservation>),
    BleAvailability(Availability),
    BleLink(ble_transport::Snapshot),
    BlePacket(ble_transport::Packet),
    BleBattery(ble_transport::BatteryLevel),
    PositionAvailability(Availability),
    Position(Option<positioning::Snapshot>),
    Startup(sensors::Startup),
    Physical(sensors::Snapshot),
    Light(Observation<sensors::Light>),
    Fitted(sensors::SensorSet),
    Companion(Option<InputSnapshot>),
    /// A hardware random value the card's owner asked for, and one the
    /// access point drew its network from.
    Entropy(u64),
    AccessPointEntropy(u64),
    /// A read, write or erase of the internal flash or the card.
    Media(StorageOp, Access<'a>),
    /// The card refused to say where it is.
    NoCard(StorageError),
    AccessPointState(AccessPointState),
    AccessPointReach(Reach),
    AccessPointRequest {
        start: bool,
        result: Result<(), Error>,
    },
    ServerRequest(Option<ServerRequest<'a>>),
    ServerRoom(usize),
    ServerListen(Listen),
    ServerLink(PhoneLink),
    ServerFinish(u32),
    ServerRead {
        id: u32,
        part: &'a [u8],
        more: bool,
    },
    ServerWrite {
        id: u32,
        taken: usize,
    },
    SoundState(sound::Snapshot),
    SoundRequest {
        play: Option<u8>,
        result: Result<(), Error>,
    },
    PowerControl(power::Status),
    PowerCapabilities(power::Capabilities),
    PowerRequest {
        operation: power::Operation,
        delay_ms: u32,
        result: Result<(), Error>,
    },
    Backlight(Availability),
    /// The panel or the backlight refused the shell at this attempt of the
    /// pass.
    Refused {
        output: Output,
        attempt: u8,
    },
    /// A record of a method the tools show nothing of.
    Other,
}

/// What composition gave the session a trace records, as it opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opened {
    pub memory: Memory,
    /// Secrets were replaced from the first record on.
    pub redacted: bool,
}

/// The session `trace` opened with. One that says nothing had every block
/// of working memory and replaced no secret.
pub fn opened(trace: &[u8]) -> Opened {
    let opened = firmware_services::trace::decode(trace)
        .ok()
        .and_then(|(_, mut events)| {
            events.find_map(|event| match event {
                Event::Session { memory, redacted } => Some(Opened { memory, redacted }),
                _ => None,
            })
        });
    opened.unwrap_or(Opened {
        memory: Memory::ALL,
        redacted: false,
    })
}

/// What `call` holds. `None` for a method no table declares, or a body that
/// is not the method's.
pub fn read<'a>(call: &Call<'a>) -> Option<World<'a>> {
    let (capability, method) = declared(call.method)?;
    let mut r = Reader::new(call.body, call.now_ms);
    let r = &mut r;
    let world = match (capability.name, method.name) {
        ("InputSource", "take_edge") => World::Edge(Wire::take(r)),
        ("Ant", "availability") => World::AntAvailability(Wire::take(r)),
        ("Ant", "scanning") => World::AntScanning(Wire::take(r)),
        ("Ant", "discoveries") => World::AntDiscovery {
            slot: Wire::take(r),
            found: Wire::take(r),
        },
        ("Ant", "channels") => World::AntLink {
            channel: Wire::take(r),
            snapshot: Wire::take(r),
        },
        ("Ant", "take_packet") => World::AntPacket(Wire::take(r)),
        ("Ant", "request") => World::AntRequest {
            operation: Wire::take(r),
            result: Wire::take(r),
        },
        ("Ant", "send_status") => {
            let _: ant::OperationId = Wire::take(r);
            World::AntSend(Wire::take(r))
        }
        ("Ble", "availability") => World::BleAvailability(Wire::take(r)),
        ("Ble", "snapshot") => World::BleLink(Wire::take(r)),
        ("Ble", "take_packet") => World::BlePacket(Wire::take(r)),
        ("Ble", "take_battery") => World::BleBattery(Wire::take(r)),
        ("Positioning", "availability") => World::PositionAvailability(Wire::take(r)),
        ("Positioning", "snapshot") => World::Position(Wire::take(r)),
        ("Sensors", "startup_status") => World::Startup(Wire::take(r)),
        ("Sensors", "snapshot") => World::Physical(Wire::take(r)),
        ("Sensors", "light") => World::Light(Wire::take(r)),
        ("Sensors", "fitted") => World::Fitted(Wire::take(r)),
        ("InputObservation", "snapshot") => World::Companion(Wire::take(r)),
        ("ReadWrite", "entropy") => World::Entropy(Wire::take(r)),
        ("AccessPoint", "entropy") => World::AccessPointEntropy(Wire::take(r)),
        ("ReadWrite", "owned_info") => {
            match <Result<bulk::OwnedInfo, bulk::Error> as Wire>::take(r) {
                Ok(_) => World::Other,
                Err(error) => World::NoCard(StorageError::from_bulk(error)),
            }
        }
        ("AccessPoint", "state") => World::AccessPointState(Wire::take(r)),
        ("AccessPoint", "reach") => World::AccessPointReach(Wire::take(r)),
        ("AccessPoint", "start" | "stop") => World::AccessPointRequest {
            start: method.name == "start",
            result: Wire::take(r),
        },
        ("Server", "request") => World::ServerRequest(Wire::take(r)),
        ("Server", "room") => World::ServerRoom(Wire::take(r)),
        ("Server", "listen") => World::ServerListen(Wire::take(r)),
        ("Server", "link") => World::ServerLink(Wire::take(r)),
        ("Server", "finish") => World::ServerFinish(Wire::take(r)),
        ("Server", "read") => {
            let (id, more) = (Wire::take(r), Wire::take(r));
            World::ServerRead {
                id,
                part: Wire::take(r),
                more,
            }
        }
        ("Server", "write") => {
            let (_, id, _): (u8, u32, usize) = Wire::take(r);
            World::ServerWrite {
                id,
                taken: Wire::take(r),
            }
        }
        ("Sound", "snapshot") => World::SoundState(Wire::take(r)),
        ("Sound", "play") => World::SoundRequest {
            play: Some(Wire::take(r)),
            result: Wire::take(r),
        },
        ("Sound", "stop") => World::SoundRequest {
            play: None,
            result: Wire::take(r),
        },
        ("PowerControl", "status") => World::PowerControl(Wire::take(r)),
        ("PowerControl", "capabilities") => World::PowerCapabilities(Wire::take(r)),
        ("PowerControl", "request") => World::PowerRequest {
            operation: Wire::take(r),
            delay_ms: 0,
            result: Wire::take(r),
        },
        ("PowerControl", "request_after") => World::PowerRequest {
            operation: Wire::take(r),
            delay_ms: Wire::take(r),
            result: Wire::take(r),
        },
        ("Power", "availability") => World::Backlight(Wire::take(r)),
        // A sink's record opens with its attempt of the pass, and is
        // written only for a refusal.
        ("Display", "submit") => {
            return Some(World::Refused {
                output: Output::Panel,
                attempt: *call.body.first()?,
            });
        }
        ("Power", "brightness") => {
            return Some(World::Refused {
                output: Output::Backlight,
                attempt: *call.body.first()?,
            });
        }
        _ => match method.kind {
            Kind::Media(verb) => {
                let op = match verb {
                    Media::Read => StorageOp::Read,
                    Media::Write => StorageOp::Write,
                    Media::Erase => StorageOp::Erase,
                };
                return Some(World::Media(op, Access::of(call)?));
            }
            _ => return Some(World::Other),
        },
    };
    (r.read() == Some(call.body.len())).then_some(world)
}
