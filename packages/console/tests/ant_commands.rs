use device_api::ant::*;
use device_api::observation::Availability;
use firmware_console::{commands::ant::execute, protocol::parse};
use firmware_services::ant::Channels;

struct Transport(Channels);
impl Ant for Transport {
    fn scanning(&self) -> bool {
        self.0.scanning()
    }
    fn discoveries(&self) -> [Option<Discovery>; 8] {
        *self.0.discoveries()
    }
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn channels(&self, now: u64) -> [Option<Snapshot>; CHANNEL_CAPACITY] {
        self.0.snapshots(now)
    }
    fn take_packet(&mut self) -> Option<Packet> {
        self.0.pop_packet()
    }
    fn take_diagnostic_packet(&mut self) -> Option<Packet> {
        self.0.pop_diagnostic_packet()
    }
    fn send_status(&self, id: OperationId) -> Option<SendObservation> {
        self.0.send_status(id)
    }
    fn request(&mut self, operation: AntOperation, now: u64) -> Result<Admission, Error> {
        match operation {
            AntOperation::Send { target, data } => self
                .0
                .queue_send(target, data, now)
                .map(Admission::SendQueued),
            _ => panic!("unexpected control command"),
        }
    }
}

#[test]
fn terminal_diagnostics_preserve_application_pages_and_bind_send_generation() {
    let identity = Identity {
        device_type: 120,
        device_number: 1234,
        transmission_type: 1,
    };
    let mut ant = Transport(Channels::new());
    ant.0.connect(identity, 0).unwrap();
    ant.0.receive(Event::Connected(identity), 1);
    ant.0.receive(
        Event::Data {
            device_type: 120,
            data: [42; 8],
        },
        2,
    );
    let mut output = String::new();
    assert_eq!(
        execute(
            parse(b"CMD 1 ANT READ").unwrap().command,
            &mut ant,
            3,
            &mut output
        ),
        Some("OK")
    );
    assert!(output.contains("42"));
    assert_eq!(ant.take_packet().unwrap().data, [42; 8]);
    assert_eq!(
        execute(
            parse(b"CMD 2 ANT READ").unwrap().command,
            &mut ant,
            3,
            &mut output
        ),
        Some("EMPTY")
    );
    let generation = ant.channel(120, 3).unwrap().generation;
    let send = format!("CMD 3 ANT SEND 120 1234 1 {generation} 0102030405060708");
    output.clear();
    assert_eq!(
        execute(
            parse(send.as_bytes()).unwrap().command,
            &mut ant,
            3,
            &mut output
        ),
        Some("ACCEPTED")
    );
    assert!(output.contains("stage=queued delivery=unknown"));
    let (id, target, data) = ant.0.pending_send().unwrap();
    assert_eq!(
        target,
        Target {
            identity,
            generation
        }
    );
    assert_eq!(data, [1, 2, 3, 4, 5, 6, 7, 8]);
    ant.0.send_submitted(id, 4);
    ant.0.send_reply(id, true, 5);
    output.clear();
    let lookup = format!("CMD 4 ANT OPERATION {}", id.0);
    assert_eq!(
        execute(
            parse(lookup.as_bytes()).unwrap().command,
            &mut ant,
            6,
            &mut output
        ),
        Some("OK")
    );
    assert!(output.contains("BridgeReplied"));
    ant.0.transport_loss(7);
    assert_eq!(
        execute(
            parse(send.as_bytes()).unwrap().command,
            &mut ant,
            8,
            &mut output
        ),
        Some("STALE")
    );
}
